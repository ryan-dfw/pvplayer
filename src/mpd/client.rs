//! A blocking TCP connection to MPD: queries, stickers, and change events.

use super::{Song, Status};
use anyhow::{Context, bail};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A connection to MPD.
///
/// Every method sends one command and waits for MPD's full reply.
pub struct MpdClient {
    /// The TCP stream, buffered so replies can be read line by line.
    reader: BufReader<TcpStream>,
}

/// Quotes-safe form of `value` for use inside a `"…"` MPD argument:
/// backslashes and double quotes are escaped.
///
/// # Errors
///
/// Returns an error if `value` contains a newline, which would end the
/// command early.
fn escape_argument(value: &str) -> anyhow::Result<String> {
    if value.contains('\n') || value.contains('\r') {
        bail!("MPD command argument contains a newline");
    }

    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl MpdClient {
    /// Sends one command line and collects reply lines until MPD's `OK`.
    ///
    /// # Errors
    ///
    /// Returns an error if writing or reading fails (including the 5 s read
    /// timeout on query connections), MPD closes the connection mid-reply,
    /// or MPD answers `ACK` (the command was rejected).
    fn command(&mut self, command: &str) -> anyhow::Result<Vec<String>> {
        let request = format!("{}\n", command);

        self.reader
            .get_mut()
            .write_all(request.as_bytes())
            .context("Couldn't write to MPD")?;

        let mut response = Vec::new();

        loop {
            let mut line = String::new();

            let bytes_read = self
                .reader
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

    /// Connects to MPD at `address` (e.g. `localhost:6600`) and reads its
    /// greeting. Replies must arrive within 5 s.
    ///
    /// MPD closes clients that stay silent for its `connection_timeout`
    /// (60 s by default), so a long-lived connection must keep sending
    /// commands. [`crate::playback::run`] does, at least every 15 s.
    ///
    /// # Errors
    ///
    /// Returns an error if the TCP connection fails, the timeout can't be
    /// set, or MPD closes the connection before greeting.
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

    /// Like [`Self::connect`], but with no read timeout, for use with
    /// [`Self::wait_for_change`], which may wait indefinitely. MPD exempts
    /// clients waiting in `idle` from its connection timeout.
    ///
    /// # Errors
    ///
    /// Same as [`Self::connect`], or if the timeout can't be removed.
    pub fn connect_for_events(address: &str) -> anyhow::Result<Self> {
        let client = Self::connect(address)?;
        client
            .reader
            .get_ref()
            .set_read_timeout(None)
            .context("Couldn't disable the MPD event read timeout")?;
        Ok(client)
    }

    /// Blocks until MPD's player (play, pause, seek, song change) or queue
    /// changes. Sends `idle player playlist`.
    ///
    /// The connection can do nothing else while waiting.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails while waiting.
    pub fn wait_for_change(&mut self) -> anyhow::Result<()> {
        self.command("idle player playlist")
            .context("Couldn't wait for an MPD change")?;
        Ok(())
    }

    /// Fetches MPD's current player state.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails (see [`Self::command`]) or the
    /// reply can't be parsed (see [`Status`]).
    pub fn get_status(&mut self) -> anyhow::Result<Status> {
        let response = self.command("status").context("Couldn't get MPD status")?;

        Status::parse(&response).context("Couldn't parse MPD status")
    }

    /// Fetches the queue entry with `songid` `id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails, including when no queue entry
    /// has that ID (e.g. it was just removed), or the reply has no `file`.
    pub fn get_song(&mut self, id: u32) -> anyhow::Result<Song> {
        let response = self
            .command(&format!("playlistid {}", id))
            .with_context(|| format!("Couldn't get song {}", id))?;

        Song::parse(&response).context("Couldn't parse song details")
    }

    /// Fetches every sticker on the song at `uri`, as raw `sticker: name=value`
    /// lines. Parsed by [`crate::pv_link::PvLink::from_stickers`].
    ///
    /// # Errors
    ///
    /// Returns an error if `uri` contains a newline, or MPD rejects the
    /// command (e.g. the file isn't in MPD's database, or stickers are
    /// disabled).
    pub fn get_stickers(&mut self, uri: &str) -> anyhow::Result<Vec<String>> {
        let uri = escape_argument(uri)?;

        self.command(&format!("sticker list song \"{uri}\""))
            .context("Couldn't get song stickers")
    }

    /// Sets sticker `name` to `value` on the song at `uri`, replacing any
    /// existing value. Used by `annotate`.
    ///
    /// # Errors
    ///
    /// Returns an error if any argument contains a newline, or MPD rejects
    /// the command (as with [`Self::get_stickers`]).
    pub fn set_sticker(&mut self, uri: &str, name: &str, value: &str) -> anyhow::Result<()> {
        let uri = escape_argument(uri)?;
        let name = escape_argument(name)?;
        let value = escape_argument(value)?;

        self.command(&format!(
            "sticker set song \"{uri}\" \"{name}\" \"{value}\""
        ))
        .with_context(|| format!("Couldn't set sticker {name}"))?;

        Ok(())
    }

    /// Pauses MPD. Not currently used by pvplayer.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails (see [`Self::command`]).
    pub fn pause(&mut self) -> anyhow::Result<()> {
        self.command("pause 1").context("Couldn't pause mpd")?;
        Ok(())
    }

    /// Resumes MPD. Not currently used by pvplayer.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails (see [`Self::command`]).
    pub fn resume(&mut self) -> anyhow::Result<()> {
        self.command("pause 0").context("Couldn't resume mpd")?;
        Ok(())
    }

    /// Seeks the current song to `seconds`. Not currently used by pvplayer.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails (see [`Self::command`]).
    pub fn seek_current(&mut self, seconds: f64) -> anyhow::Result<()> {
        self.command(&format!("seekcur {seconds:.6}"))
            .context("Couldn't seek current MPD song")?;

        Ok(())
    }
}
