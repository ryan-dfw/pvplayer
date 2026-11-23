//! Spawns a muted mpv and controls it over its JSON IPC socket.
//!
//! mpv is started once, idle and paused, and reused for every video.
//! Commands are JSON lines sent over a Unix socket; see mpv's
//! [JSON IPC docs](https://mpv.io/manual/stable/#json-ipc).

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use std::{fs, thread};

/// Where mpv's IPC socket is created.
///
/// Fixed, so only one pvplayer can run at a time: [`Mpv::start`] deletes
/// any existing socket here, cutting off another instance's mpv.
const SOCKET_PATH: &str = "/tmp/pvplay-mpv.sock";

/// A running mpv process and the IPC connection that controls it.
///
/// Dropping it kills mpv and removes the socket.
pub struct Mpv {
    /// The mpv process, kept so it can be killed on drop.
    child: Child,
    /// The IPC connection. Replies and events arrive on it as JSON lines.
    socket: BufReader<UnixStream>,
    /// ID for the next request; mpv echoes it back so replies can be matched.
    next_request_id: u64,
    /// `file-loaded` events seen while waiting for other replies, counted so
    /// [`Self::wait_until_file_loaded`] doesn't miss them.
    pending_file_loaded: usize,
}

impl Mpv {
    /// Spawns mpv and connects to its IPC socket.
    ///
    /// mpv starts with no file, paused and muted (`--no-audio`), with its
    /// keyboard shortcuts and on-screen controls disabled so the window
    /// can't be paused or seeked by accident. Only warnings are printed.
    ///
    /// Waits up to about 1 s for the socket to appear. Replies are then
    /// expected within 5 s.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - a stale socket from an earlier run can't be removed
    /// - mpv can't be started (e.g. not installed or not on `PATH`)
    /// - mpv doesn't create its socket within about 1 s
    /// - the socket's timeouts can't be set
    pub fn start() -> Result<Self> {
        let socket_path = Path::new(SOCKET_PATH);

        // clean up if exists earlier crashed run
        if socket_path.exists() {
            fs::remove_file(socket_path).context("Couldn't remove stale mpv IPC socket")?;
        }

        let child = Command::new("mpv")
            .arg("--no-audio")
            .arg("--idle=yes")
            .arg("--pause")
            .arg(format!("--input-ipc-server={SOCKET_PATH}"))
            .arg("--msg-level=all=warn")
            .arg("--no-input-default-bindings")
            .arg("--no-osc")
            .stdin(Stdio::null())
            .spawn()
            .context("Couldn't start mpv")?;

        let mut socket = None;

        for _ in 0..50 {
            match UnixStream::connect(socket_path) {
                Ok(stream) => {
                    socket = Some(stream);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }

        let socket = socket.context("mpv did not create its IPC socket")?;

        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .context("Couldn't set IPC read timeout")?;

        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .context("Couldn't set IPC write timeout")?;

        Ok(Self {
            child,
            socket: BufReader::new(socket),
            next_request_id: 1,
            pending_file_loaded: 0,
        })
    }

    /// Loads `path`, replacing any current video, and leaves it paused at
    /// the start. Returns once mpv reports the file loaded.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `path` is not valid UTF-8
    /// - mpv rejects the pause or load command
    /// - mpv never reports the file loaded (e.g. the file is missing). The
    ///   read times out after 5 s.
    pub fn load_paused(&mut self, path: &Path) -> Result<()> {
        let path = path.to_str().context("PV path is not valid UTF-8")?;

        self.send(json!(["set_property", "pause", true]))
            .context("Couldn't pause mpv before loading")?;

        self.send(json!(["loadfile", path, "replace"]))
            .context("Couldn't ask mpv to load {path}")?;

        self.wait_until_file_loaded()
            .with_context(|| format!("mpv couldn't finish loading {path}"))?;

        Ok(())
    }

    /// Sends one command and returns mpv's reply to it.
    ///
    /// Events that arrive first are skipped, except `file-loaded`, which is
    /// counted in [`Self::pending_file_loaded`]. Replies to other requests
    /// are skipped by `request_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket can't be written or read (including
    /// the 5 s timeout), mpv closes it, a reply isn't valid JSON, or mpv
    /// answers with anything other than `success`.
    fn request(&mut self, command: Value) -> Result<Value> {
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let request = json!({
            "command": command,
            "request_id": request_id
        });

        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');

        self.socket
            .get_mut()
            .write_all(&bytes)
            .context("Couldn't send command to mpv")?;

        loop {
            let mut line = String::new();

            let bytes_read = self
                .socket
                .read_line(&mut line)
                .context("Couldn't read mpv response")?;

            if bytes_read == 0 {
                bail!("MPV closed its IPC connection");
            }

            let response: Value =
                serde_json::from_str(&line).context("mpv returned invalid JSON")?;

            if let Some(event) = response.get("event").and_then(Value::as_str) {
                if event == "file-loaded" {
                    self.pending_file_loaded += 1;
                }

                continue;
            }

            if response.get("request_id").and_then(Value::as_u64) != Some(request_id) {
                continue;
            }

            let error = response
                .get("error")
                .and_then(Value::as_str)
                .context("mpv response is missing its error status")?;

            if error != "success" {
                bail!("mpv rejected the command: {}", error);
            }

            return Ok(response);
        }
    }

    /// Like [`Self::request`], discarding the reply.
    ///
    /// # Errors
    ///
    /// Same as [`Self::request`].
    fn send(&mut self, command: Value) -> Result<()> {
        self.request(command).map(|_| ())
    }

    /// Blocks until mpv emits `file-loaded`, or uses one already counted.
    ///
    /// # Errors
    ///
    /// Returns an error if reading fails or times out after 5 s, mpv closes
    /// the socket, or an event isn't valid JSON.
    fn wait_until_file_loaded(&mut self) -> Result<()> {
        if self.pending_file_loaded > 0 {
            self.pending_file_loaded -= 1;
            return Ok(());
        }

        loop {
            let mut line = String::new();

            let bytes_read = self
                .socket
                .read_line(&mut line)
                .context("Couldn't read mpv event")?;

            if bytes_read == 0 {
                bail!("mpv closed IPC while loading a file");
            }

            let message: Value =
                serde_json::from_str(&line).context("mpv returned invalid JSON")?;

            let Some(event) = message.get("event").and_then(Value::as_str) else {
                continue;
            };

            if event == "file-loaded" {
                return Ok(());
            }
        }
    }

    /// Unpauses the video.
    ///
    /// # Errors
    ///
    /// Same as [`Self::request`].
    pub fn play(&mut self) -> Result<()> {
        self.send(json!(["set_property", "pause", false]))
    }

    /// Pauses the video.
    ///
    /// # Errors
    ///
    /// Same as [`Self::request`].
    pub fn pause(&mut self) -> Result<()> {
        self.send(json!(["set_property", "pause", true]))
    }

    /// Seeks to `seconds` into the video, frame-accurately (`absolute+exact`).
    /// Exact seeks are slower than keyframe seeks, but land where asked.
    ///
    /// # Errors
    ///
    /// Same as [`Self::request`].
    pub fn seek_absolute(&mut self, seconds: f64) -> Result<()> {
        self.send(json!(["seek", seconds, "absolute+exact"]))
    }

    /// The video's current position in seconds (mpv's `time-pos`).
    ///
    /// # Errors
    ///
    /// Same as [`Self::request`], or if mpv's reply has no numeric position
    /// (e.g. no file is loaded).
    pub fn time_position(&mut self) -> Result<f64> {
        let response = self.request(json!(["get_property", "time-pos"]))?;

        response
            .get("data")
            .and_then(Value::as_f64)
            .context("mpv response is missing a numeric time-pos")
    }

    /// mpv's audio position in seconds (`audio-pts`), if it has one.
    ///
    /// Always `None` in pvplayer: mpv runs with `--no-audio`, so it has no
    /// audio position. Used only by the diagnostic `JOIN CHECK` line.
    pub fn audio_position(&mut self) -> Option<f64> {
        let response = self.request(json!(["get_property", "audio-pts"])).ok()?;

        response.get("data").and_then(Value::as_f64)
    }
}

/// Kills mpv and removes its socket.
///
/// Doesn't run if pvplayer is killed by a signal (e.g. Ctrl-C); the
/// leftover socket is removed by the next [`Mpv::start`].
impl Drop for Mpv {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(SOCKET_PATH);
    }
}
