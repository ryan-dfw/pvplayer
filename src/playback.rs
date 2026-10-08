use std::thread;
use std::time::Duration;
use std::sync::mpsc::{self, RecvTimeoutError};

use crate::mpv::Mpv;
use crate::mpd::{MpdClient, PlaybackState, Status};
use crate::pv_link::PvLink;
use crate::timing::seconds_until_pv;

use anyhow::Result;

#[derive(Debug, PartialEq, Eq)]
enum VideoState {
    Playing,
    Paused
}

#[derive(Debug)]
enum PvPhase {
    Empty,
    Staged {
        target_song_id: u32,
        link: PvLink,
    },
    Active {
        target_song_id: u32,
        start_offset_ms: i64,
        video_state: VideoState
    }
}

enum MpdEvent {
    Changed,
    Failed(anyhow::Error)
}

fn start_mpd_event_listener(
    address: &str,
) -> Result<mpsc::Receiver<MpdEvent>> {
    let mut event_client = MpdClient::connect_for_events(address)?;
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || loop {
        match event_client.wait_for_change() {
            Ok(()) => {
                if sender.send(MpdEvent::Changed).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(MpdEvent::Failed(error));
                break;
            }
         }
    });

    Ok(receiver)
}

fn intended_video_position(
    status: &Status,
    target_song_id: u32,
    start_offset_ms: i64,
) -> Option<f64> {
    let offset_seconds = start_offset_ms as f64 / 1000.0;

    let position = if status.song_id == Some(target_song_id) {
        status.elapsed?.as_secs_f64() + offset_seconds
    } else if status.next_song_id == Some(target_song_id) {
        status.elapsed?.as_secs_f64() + offset_seconds - status.duration?.as_secs_f64()
    } else {
        return None;
    };

    (position >= 0.0).then_some(position)
}

pub fn run() -> Result<()> {
    let mut client = MpdClient::connect("localhost:6600")?;
    let mpd_events = start_mpd_event_listener("localhost:6600")?;
    let mut mpv = Mpv::start()?;
    let mut previous_next_id: Option<u32> = None;
    let mut pv_phase = PvPhase::Empty;
    let mut previous_display = String::new();

    loop {
        let status = client.get_status()?;

        let active_pv_is_relevant = matches!(
            &pv_phase,
            PvPhase::Active { target_song_id, .. }
                if Some(*target_song_id) == status.song_id
                    || Some(*target_song_id) == status.next_song_id
        );

        if matches!(&pv_phase, PvPhase::Active { .. })
            && !active_pv_is_relevant {
            pv_phase = PvPhase::Empty;
        }

        if status.next_song_id != previous_next_id {
            previous_display.clear();

            let active_pv_is_for_current_song = matches!(
                &pv_phase,
                PvPhase::Active { target_song_id, .. }
                    if Some(*target_song_id) == status.song_id
            );

            if active_pv_is_for_current_song {
                println!("Keeping the current PV active, not preloading another");
            } else {
                pv_phase = PvPhase::Empty;

                if let Some(id) = status.next_song_id {
                    let song = client.get_song(id)?;

                    println!(
                        "Next up: {}",
                        song.title.as_deref().unwrap_or(&song.file)
,                    );

                    let stickers = client.get_stickers(&song.file)?;

                    if let Some(link) =
                        PvLink::from_stickers(&song.file, &stickers)
                    {
                        println!("Preloading PV: {}", link.path.display());
                        mpv.load_paused(&link.path)?;

                        pv_phase = PvPhase::Staged {
                            target_song_id: id,
                            link
                        };
                    } else {
                        println!("No playable PV link");
                    }
                } else {
                    println!("No next song reported")
                }
            }
            previous_next_id = status.next_song_id;
        }


        let mut should_start = false;

        if let PvPhase::Staged { link, .. } = &pv_phase {
            if let(Some(duration), Some(elapsed)) =
                (status.duration, status.elapsed) {
                let remaining = seconds_until_pv(
                    duration,
                    elapsed,
                    link.start_offset_ms
                );

                should_start = remaining <= 0.0
                    && matches!(
                        &status.state,
                        Some(PlaybackState::Playing)
                    )
                    && matches!(
                        &pv_phase,
                        PvPhase::Staged { .. }
                    );


                let activity = match &status.state {
                    Some(PlaybackState::Playing) => Some("Playing"),
                    Some(PlaybackState::Paused) => Some("Paused"),
                    _ => None,
                };

                if let Some(activity) = activity {
                    let display = if remaining >= 0.0 {
                        format!(
                            "{} - PV begins in {:.0} playback seconds",
                            activity,
                            remaining.ceil(),
                        )
                    } else {
                        format!(
                            "{} - intended PV position: {:.0} seconds",
                            activity,
                            (-remaining).floor(),
                        )
                    };

                    if display != previous_display {
                        println!("{}", display);
                        previous_display = display;
                    }
                }
            }
        }

        if should_start {
            mpv.play()?;

            pv_phase = match pv_phase {
                PvPhase::Staged {
                    target_song_id,
                    link
                } => PvPhase::Active {
                    target_song_id,
                    start_offset_ms: link.start_offset_ms,
                    video_state: VideoState::Playing
                },
                other => other,
            };

            println!("Started PV");
        }

        match (&mut pv_phase, &status.state) {
            (
                PvPhase::Active {
                    target_song_id,
                    start_offset_ms,
                    video_state
                },
                Some(PlaybackState::Paused),
            ) if *video_state == VideoState::Playing => {
                mpv.pause()?;

                if let Some(position) = intended_video_position(
                    &status,
                    *target_song_id,
                    *start_offset_ms,
                ) { mpv.seek_absolute(position)?; }

                *video_state = VideoState::Paused;
                println!("Paused PV");
            }

            (
                PvPhase::Active { video_state, .. },
                Some(PlaybackState::Playing),
            ) if *video_state == VideoState::Paused => {
                mpv.play()?;
                *video_state = VideoState::Playing;
                println!("Resumed PV");
            }

            _ => {}
        }

        match mpd_events.recv_timeout(Duration::from_millis(500)) {
            Ok(MpdEvent::Changed) => {}

            Ok(MpdEvent::Failed(err)) => {
                return Err(err);
            }

            Err(RecvTimeoutError::Timeout)  => {}

            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("MPD event listener stopped unexpectedly")
            }
        }
    }
}