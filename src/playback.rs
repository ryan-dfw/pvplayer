use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::mpd::{MpdClient, PlaybackState, Status};
use crate::mpv::Mpv;
use crate::pv_link::PvLink;
use crate::timing::seconds_until_pv;

use anyhow::Result;

const START_TIMING_RESOLUTION: Duration = Duration::from_millis(20);
const MAX_RECHECK_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq, Eq)]
enum VideoState {
    Playing,
    Paused,
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
        video_state: VideoState,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum NextSongUpdate {
    Unchanged,
    KeepActive,
    Load(u32),
    NoNextSong,
}

#[derive(Debug, PartialEq)]
enum PlaybackAction {
    Start,
    Pause { seek_to: Option<f64> },
    Resume
}

#[derive(Debug)]
struct PlaybackController {
    previous_next_id: Option<u32>,
    phase: PvPhase,
    previous_display: String,
}

impl PlaybackController {
    fn new() -> Self {
        Self {
            previous_next_id: None,
            phase: PvPhase::Empty,
            previous_display: String::new(),
        }
    }

    fn observe_next_song(&mut self, status: &Status) -> NextSongUpdate {
        let active_is_relevant = matches!(
            &self.phase,
            PvPhase::Active{ target_song_id, ..}
                if Some(*target_song_id) == status.song_id
                    || Some(*target_song_id) == status.next_song_id
        );

        if matches!(&self.phase, PvPhase::Active { .. }) && !active_is_relevant {
            self.phase = PvPhase::Empty;
        }

        if status.next_song_id == self.previous_next_id {
            return NextSongUpdate::Unchanged;
        }

        self.previous_display.clear();
        self.previous_next_id = status.next_song_id;

        let active_is_current = matches!(
            &self.phase,
            PvPhase::Active{ target_song_id, ..}
                if Some(*target_song_id) == status.song_id
        );

        if active_is_current {
            return NextSongUpdate::KeepActive;
        }

        self.phase = PvPhase::Empty;

        match status.next_song_id {
            Some(id) => NextSongUpdate::Load(id),
            None => NextSongUpdate::NoNextSong,
        }
    }

    fn stage(&mut self, target_song_id: u32, link: PvLink) {
        self.phase = PvPhase::Staged {
            target_song_id,
            link,
        };
    }

    fn staged_seconds_until_start(&self, status: &Status) -> Option<f64> {
        let PvPhase::Staged{ link, .. } = &self.phase else { return None } ;
        let duration = status.duration?;
        let elapsed = status.elapsed?;

        Some(seconds_until_pv(
            duration,
            elapsed,
            link.start_offset_ms
        ))
    }

    fn next_check_delay(&self, status: &Status) -> Duration {
        if !matches!(&status.state, Some(PlaybackState::Playing)) {
            return MAX_RECHECK_INTERVAL;
        }

        let Some(remaining) = self.staged_seconds_until_start(status) else {
            return MAX_RECHECK_INTERVAL;
        };

        if remaining <= 0.0 {
            return START_TIMING_RESOLUTION;
        }

        Duration::from_secs_f64(remaining / 2.0)
            .clamp(START_TIMING_RESOLUTION, MAX_RECHECK_INTERVAL)
    }

    fn display_update(&mut self, status: &Status) -> Option<String>{
        let remaining = self.staged_seconds_until_start(status)?;

        let activity = match &status.state {
            Some(PlaybackState::Playing) => "Playing",
            Some(PlaybackState::Paused) => "Paused",
            _ => return None
        };

        let display = if remaining >= 0.0 {
            format!(
                "{} - PV begins in {:.0} playback seconds",
                activity, remaining.ceil()
            )
        } else {
            format!(
                "{} - intended PV position: {:.0} seconds",
                activity, (-remaining).floor()
            )
        };

        if display == self.previous_display {
            return None;
        }

        self.previous_display = display.clone();
        Some(display)
    }

    fn next_action(&mut self, status: &Status) -> Option<PlaybackAction> {
        let should_start = matches!(
            &status.state,
            Some(PlaybackState::Playing)
        ) && self
            .staged_seconds_until_start(status)
            .is_some_and(|remaining| remaining <= 0.0);

        if should_start {
            let phase = std::mem::replace(
                &mut self.phase,
                PvPhase::Empty
            );

            self.phase = match phase {
                PvPhase::Staged {
                    target_song_id,
                    link
                } => PvPhase::Active {
                    target_song_id,
                    start_offset_ms: link.start_offset_ms,
                    video_state: VideoState::Playing,
                },
                other => other
            };

            return Some(PlaybackAction::Start);
        }

        match (&mut self.phase, &status.state) {
            (
                PvPhase::Active {
                    target_song_id,
                    start_offset_ms,
                    video_state,
                },
                Some(PlaybackState::Paused),
            ) if *video_state == VideoState::Playing => {
                let seek_to = intended_video_position(
                    status,
                    *target_song_id,
                    *start_offset_ms
                );

                *video_state = VideoState::Paused;

                Some(PlaybackAction::Pause { seek_to})
            }

            (
                PvPhase::Active { video_state, .. },
                Some(PlaybackState::Playing),
            ) if *video_state == VideoState::Paused => {
                *video_state = VideoState::Playing;
                Some(PlaybackAction::Resume)
            }

            _ => None,
        }
    }
}

enum MpdEvent {
    Changed,
    Failed(anyhow::Error),
}

fn start_mpd_event_listener(address: &str) -> Result<mpsc::Receiver<MpdEvent>> {
    let mut event_client = MpdClient::connect_for_events(address)?;
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        loop {
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
    let mut controller = PlaybackController::new();

    loop {
        let status = client.get_status()?;

        match controller.observe_next_song(&status) {
            NextSongUpdate::Unchanged => {}

            NextSongUpdate::KeepActive => {
                println!("Keeping the current PV active, not preloading another");
            }

            NextSongUpdate::Load(id) => {
                let song = client.get_song(id)?;

                println!("Next up: {}", song.title.as_deref().unwrap_or(&song.file));

                let stickers = client.get_stickers(&song.file)?;

                if let Some(link) = PvLink::from_stickers(&song.file, &stickers) {
                    println!("Preloading PV: {}", link.path.display());
                    mpv.load_paused(&link.path)?;
                    controller.stage(id, link);
                } else {
                    println!("No playable PV link");
                }
            }

            NextSongUpdate::NoNextSong => {
                println!("No next song reported");
            }
        }

        if let Some(display) = controller.display_update(&status) {
            println!("{}", display);
        }

        if let Some(action) = controller.next_action(&status) {
            match action {
                PlaybackAction::Start => {
                    mpv.play()?;
                    println!("Started PV");
                }

                PlaybackAction::Pause { seek_to } => {
                    mpv.pause()?;

                    if let Some(position) = seek_to {
                        mpv.seek_absolute(position)?;
                    }

                    println!("Paused PV");
                }

                PlaybackAction::Resume => {
                    mpv.play()?;
                    println!("Resumed PV");
                }
            }
        }

        let next_check_delay = controller.next_check_delay(&status);
        match mpd_events.recv_timeout(next_check_delay) {
            Ok(MpdEvent::Changed) => {}

            Ok(MpdEvent::Failed(err)) => {
                return Err(err);
            }

            Err(RecvTimeoutError::Timeout) => {}

            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("MPD event listener stopped unexpectedly")
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use super::{intended_video_position, NextSongUpdate, PlaybackAction, PlaybackController, PvPhase, VideoState};
    use crate::mpd::{PlaybackState, Status};
    use std::time::Duration;
    use crate::pv_link::PvLink;

    #[test]
    fn calculates_position_for_current_song() {
        let status = Status {
            song_id: Some(42),
            elapsed: Some(Duration::from_secs_f64(12.5)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), Some(15.5));
    }

    #[test]
    fn calculates_position_during_preroll() {
        let status = Status {
            next_song_id: Some(42),
            elapsed: Some(Duration::from_secs(238)),
            duration: Some(Duration::from_secs(240)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), Some(1.0));
    }

    #[test]
    fn returns_none_before_preroll_begins() {
        let status = Status {
            next_song_id: Some(42),
            elapsed: Some(Duration::from_secs(230)),
            duration: Some(Duration::from_secs(240)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), None,);
    }

    #[test]
    fn returns_none_for_unrelated_song() {
        let status = Status {
            song_id: Some(7),
            next_song_id: Some(8),
            elapsed: Some(Duration::from_secs(10)),
            duration: Some(Duration::from_secs(200)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), None,);
    }

    #[test]
    fn keeps_active_pv_when_target_song_becomes_current() {
        let mut controller = PlaybackController::new();
        controller.previous_next_id = Some(42);
        controller.phase = PvPhase::Active {
            target_song_id: 42,
            start_offset_ms: 3_000,
            video_state: VideoState::Playing,
        };

        let status = Status {
            song_id: Some(42),
            next_song_id: Some(99),
            ..Status::default()
        };

        assert_eq!(
            controller.observe_next_song(&status),
            NextSongUpdate::KeepActive,
        );
        assert!(matches!(controller.phase, PvPhase::Active { .. }));
    }

    #[test]
    fn clears_active_pv_after_target_song_is_no_longer_relevant() {
        let mut controller = PlaybackController::new();
        controller.previous_next_id = Some(100);
        controller.phase = PvPhase::Active {
            target_song_id: 42,
            start_offset_ms: 3_000,
            video_state: VideoState::Playing,
        };

        let status = Status {
            song_id: Some(99),
            next_song_id: Some(100),
            ..Status::default()
        };

        assert_eq!(
            controller.observe_next_song(&status),
            NextSongUpdate::Unchanged
        );
        assert!(matches!(controller.phase, PvPhase::Empty));
    }

    #[test]
    fn handles_start_pause_resume_flow() {
        let mut controller = PlaybackController::new();

        controller.stage(
            42,
            PvLink {
                path: PathBuf::from("/test/pv.mp4"),
                start_offset_ms: 3_000,
                video_cutoff_ms: None,
            },
        );

        let ready_to_start = Status {
            duration: Some(Duration::from_secs(240)),
            elapsed: Some(Duration::from_secs(238)),
            state: Some(PlaybackState::Playing),
            ..Status::default()
        };

        assert_eq!(
        controller.next_action(&ready_to_start),
        Some(PlaybackAction::Start),
    );

        let paused = Status {
            song_id: Some(42),
            elapsed: Some(Duration::from_secs(10)),
            state: Some(PlaybackState::Paused),
            ..Status::default()
        };

        assert_eq!(
        controller.next_action(&paused),
        Some(PlaybackAction::Pause {
            seek_to: Some(13.0),
        }),
    );

        let resumed = Status {
            song_id: Some(42),
            state: Some(PlaybackState::Playing),
            ..Status::default()
        };

        assert_eq!(
        controller.next_action(&resumed),
        Some(PlaybackAction::Resume),
    );
    }
}
