use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, thread};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const SOCKET_PATH: &str = "/tmp/pvplay-mpv.sock";

pub struct Mpv {
    child: Child,
    socket: BufReader<UnixStream>,
    next_request_id: u64,
}

impl Mpv {
    pub fn start() -> Result<Self> {
        let socket_path = Path::new(SOCKET_PATH);

        // clean up if exists earlier crashed run
        if socket_path.exists() {
            fs::remove_file(socket_path)
                .context("Couldn't remove stale mpv IPC socket")?;
        }

        let child = Command::new("mpv")
            .arg("--idle=yes")
            .arg("--pause")
            .arg(format!("--input-ipc-server={SOCKET_PATH}"))
            .arg("--msg-level=all=warn")
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
        })
    }

    pub fn load_paused(&mut self, path: &Path) -> Result<()> {
        let path = path
            .to_str()
            .context("PV path is not valid UTF-8")?;

        self.send(json!(["set_property", "pause", true]))?;
        self.send(json!(["loadfile", path, "replace"]))?;

        Ok(())
    }

    fn send(&mut self, command: Value) -> Result<()> {
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

            let bytes_read = self.socket
                .read_line(&mut line)
                .context("Couldn't read mpv response")?;

            if bytes_read == 0 {
                bail!("MPV closed its IPC connection");
            }

            let response: Value = serde_json::from_str(&line)
                .context("mpv returned invalid JSON")?;

            if response.get("event").is_some() {
                continue;
            }

            if response.get("request_id").and_then(Value::as_u64)
                != Some(request_id)
            {
                continue;
            }

            let error = response
                .get("error")
                .and_then(Value::as_str)
                .context("mpv response is missing its error status")?;

            if error != "success" {
                bail!("mpv rejected the command: {}", error);
            }

            return Ok(());
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(SOCKET_PATH);
    }
}