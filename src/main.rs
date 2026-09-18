use anyhow::Result;
use std::process::ExitCode;
use pvplay::mpd::MpdClient;

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
        }
        None => println!("No next song reported"),
    }

    Ok(())
}
