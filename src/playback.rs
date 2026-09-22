use std::thread;
use std::time::Duration;

use crate::mpv::Mpv;
use crate::mpd::{MpdClient, PlaybackState};
use crate::pv_link::PvLink;
use crate::timing::seconds_until_pv;

use anyhow::Result;

pub fn run() -> Result<()> {
    let mut client = MpdClient::connect("localhost:6600")?;
    let mut mpv = Mpv::start()?;
    let mut previous_next_id: Option<u32> = None;
    let mut upcoming_pv: Option<PvLink> = None;
    let mut previous_display = String::new();

    loop {
        let status = client.get_status()?;
        if status.next_song_id != previous_next_id {
            upcoming_pv = None;

            if let Some(id) = status.next_song_id {
                let song = client.get_song(id)?;

                println!(
                    "Next up: {}",
                    song.title.as_deref().unwrap_or(&song.file)
                );

                let stickers = client.get_stickers(&song.file)?;
                upcoming_pv = PvLink::from_stickers(&song.file, &stickers);

                if let Some(pv) = &upcoming_pv {
                    println!("Preloading PV: {}", pv.path.display());
                    mpv.load_paused(&pv.path)?;
                } else {
                    println!("No playable PV link")
                }
            } else {
                println!("No next song reported");
            }

            previous_next_id = status.next_song_id;
        }

        if let Some(pv) = &upcoming_pv {
            if let(Some(duration), Some(elapsed)) =
                (status.duration, status.elapsed) {
                let remaining = seconds_until_pv(
                    duration,
                    elapsed,
                    pv.start_offset_ms
                );

                let activity = match status.state {
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

        thread::sleep(Duration::from_millis(500));
    }
}