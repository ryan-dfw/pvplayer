use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;
use anyhow::{bail, Context, Result};

pub struct MpdClient {
    reader: BufReader<TcpStream>,
}

#[derive(Debug, Default)]
pub struct Status {
    pub sample_rate_hz: Option<u32>,
    pub next_song_id: Option<u32>
}

#[derive(Debug)]
pub struct Song {
    pub file: String,
    pub title: Option<String>
}

impl Song {
    fn parse(lines: &[String]) -> Result<Self> {
        let mut file = None;
        let mut title = None;

        for line in lines {
            let line = line.strip_suffix('\n').unwrap_or(line);

            if let Some(value) = line.strip_prefix("file: ") {
                file = Some(value.to_owned());
            }

            if let Some(value) = line.strip_prefix("Title: ") {
                title = Some(value.to_owned());
            }
        }

        Ok(Self {
            file: file.context("MPD song response is missing its file")?,
            title
        })
    }
}

impl Status {
    fn parse(lines: &[String]) -> Result<Self> {
        let mut status = Self::default();

        for line in lines{
            if let Some(value) = line.strip_prefix("nextsongid: ") {
                status.next_song_id = Some(
                    value.trim().parse::<u32>()
                        .context("MPD returned an invalid next song id")?,
                );
            }

            if let Some(value) = line.strip_prefix("audio: ") {
                let (sample_rate, _) = value
                    .trim()
                    .split_once(':')
                    .context("MPD audio field is missing its separator")?;

                status.sample_rate_hz = Some(
                    sample_rate.parse::<u32>()
                        .context("MPD returned an invalid sample rate")?,
                );
            }
        }

        Ok(status)
    }
}

impl MpdClient {
    fn command(&mut self, command: &str) -> Result<Vec<String>> {
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

    pub fn connect(address: &str) -> Result<Self> {
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

    pub fn get_status(&mut self) -> Result<Status> {
        let response = self.command("status")
            .context("Couldn't get MPD status")?;

        Status::parse(&response)
            .context("Couldn't parse MPD status")
    }

    pub fn get_song(&mut self, id: u32) -> Result<Song> {
        let response = self.command(&format!("playlistid {}", id))
            .with_context(|| format!("Couldn't get song {}", id))?;

        Song::parse(&response)
            .context("Couldn't parse song details")
    }
}

#[cfg(test)]
mod tests {
    use super::{Song, Status};

    #[test]
    fn parses_song_details() {
        let lines = vec![
            String::from("file: music/example.mp3\n"),
            String::from("Title: Example Song\n"),
            String::from("Id: 2\n"),
        ];

        let song = Song::parse(&lines)
            .expect("valid song details should parse");

        assert_eq!(song.file, "music/example.mp3");
        assert_eq!(song.title.as_deref(), Some("Example Song"));
    }

    #[test]
    fn parses_status_fields() {
        let lines = vec![
            String::from("volume: 100\n"),
            String::from("audio: 44100:16:2\n"),
            String::from("nextsongid: 2\n"),
        ];

        let status = Status::parse(&lines).expect("valid status should parse");

        assert_eq!(status.sample_rate_hz, Some(44100));
        assert_eq!(status.next_song_id, Some(2));
    }

    #[test]
    fn missing_fields_remain_none() {
        let lines = vec![String::from("state: stop\n")];

        let status = Status::parse(&lines).expect("missing fields are allowed");

        assert_eq!(status.sample_rate_hz, None);
        assert_eq!(status.next_song_id, None);
    }

    #[test]
    fn rejects_invalid_sample_rate() {
        let lines = vec![String::from("audio: nope:16:2\n")];

        assert!(Status::parse(&lines).is_err());
    }

    #[test]
    fn rejects_invalid_next_song_id() {
        let lines = vec![String::from("nextsongid: nope\n")];

        assert!(Status::parse(&lines).is_err());
    }
}