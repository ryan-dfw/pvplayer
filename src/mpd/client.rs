use super::{Song, Status};
use anyhow::{Context, bail};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct MpdClient {
    reader: BufReader<TcpStream>,
}

impl MpdClient {
    fn command(&mut self, command: &str) -> anyhow::Result<Vec<String>> {
        let request = format!("{}\n", command);

        self.reader
            .get_mut()
            .write_all(request.as_bytes())
            .context("Couldn't write to MPD")?;

        let mut response = Vec::new();

        loop {
            let mut line = String::new();

            let bytes_read = self.reader
                .read_line(&mut line)
                .context("Couldn't read from MPD")?;

            if bytes_read == 0 {
                bail!("MPD closed the connection before finishing its response")
            }

            if line == "OK\n" {
                return Ok(response);
            }

            if line.starts_with("ACK ") {
                bail!("MPD rejected the command: {}", line.trim_end());
            }

            response.push(line);
        }
    }

    pub fn connect(address: &str) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(address)
            .with_context(|| format!("Failed to connect to MPD server: {}", address))?;

        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .context("Couldn't set MPD read timeout")?;

        let mut reader = BufReader::new(stream);
        let mut greeting = String::new();

        let bytes_read = reader
            .read_line(&mut greeting)
            .context("Couldn't read MPD's greeting")?;

        if bytes_read == 0 {
            bail!("MPD closed the connection before sending a greeting");
        }

        Ok(Self { reader })
    }

    pub fn get_status(&mut self) -> anyhow::Result<Status> {
        let response = self.command("status")
            .context("Couldn't get MPD status")?;

        Status::parse(&response)
            .context("Couldn't parse MPD status")

    }

    pub fn get_song(&mut self, id: u32) -> anyhow::Result<Song> {
        let response = self.command(&format!("playlistid {}", id))
            .with_context(|| format!("Couldn't get song {}", id))?;

        Song::parse(&response)
            .context("Couldn't parse song details")
    }

    pub fn get_stickers(&mut self, uri: &str) -> anyhow::Result<Vec<String>> {
        if uri.contains('\n') || uri. contains('\r') {
            bail!("URI contains a newline");
        }

        let escaped_ui = uri
            .replace('\\', "\\\\")
            .replace('"', "\\\"");

        self.command(&format!("sticker list song \"{}\"", escaped_ui))
            .context("Couldn't get song stickers")
    }
}