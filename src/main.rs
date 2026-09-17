use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let stream = TcpStream::connect("localhost:6600")
        .context("Couldn't connect to MPD")?;

    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("Couldn't set MPD read timeout")?;

    let mut reader = BufReader::new(stream);
    let mut greeting= String::new();

    let bytes_read = reader
        .read_line(&mut greeting)
        .context("Couldn't read MPD's greeting")?;

    if bytes_read == 0 {
        bail!("MPD closed the connection before sending a greeting");
    }

    print!("{}", greeting);

    Ok(())
}
