use pvplay::mpd::client::MpdClient;
use pvplay::pv_link::PvLink;
use pvplay::timing::seconds_until_pv;

use std::process::ExitCode;
use anyhow::Result;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{:#}", e);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut client = MpdClient::connect("localhost:6600")?;
    let status = client.get_status()?;

    println!("{:?}", status);

    match status.next_song_id {
        Some(id) => {
            let song = client.get_song(id)?;

            let title = song.title
                .as_deref()
                .unwrap_or(&song.file);

            println!("Next up: {}", title);

            let stickers = client.get_stickers(&song.file)?;

            match PvLink::from_stickers(&song.file, &stickers) {
                Some(pv) => {
                    println!("PV: {}", pv.path.display());
                    println!("Start offset: {} ms", pv.start_offset_ms);

                    if let (Some(duration), Some(elapsed)) = (status.duration, status.elapsed) {
                        let remaining = seconds_until_pv(
                            duration,
                            elapsed,
                            pv.start_offset_ms,
                        );

                        println!("Playback seconds until PV: {:.3}", remaining);
                    }
                }
                None => println!("No playable PV link"),
            }
        }
        None => println!("No next song reported"),
    }

    Ok(())
}
