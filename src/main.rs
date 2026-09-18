use anyhow::Result;
use pvplay::mpd::client::MpdClient;
use pvplay::pv_link::PvLink;
use std::process::ExitCode;

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
                }
                None => println!("No playable PV link"),
            }
        }
        None => println!("No next song reported"),
    }

    Ok(())
}
