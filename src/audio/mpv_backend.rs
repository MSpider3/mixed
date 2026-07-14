#![cfg(target_os = "android")]

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub struct MpvBackend {
    child: std::process::Child,
    stream: std::os::unix::net::UnixStream,
    socket_path: String,
    incoming_data: Vec<u8>,

    // Polled state
    elapsed: Duration,
    duration: Option<Duration>,
    is_paused: bool,
    idle_active: bool,
    volume: u8,
    ignore_idle: bool,
}

impl MpvBackend {
    pub fn new() -> Option<Self> {
        let prefix = std::env::var("PREFIX")
            .unwrap_or_else(|_| "/data/data/com.termux/files/usr".to_string());
        let socket_path = format!("{}/tmp/mpv_mixed.sock", prefix);

        // Ensure parent directory exists
        if let Some(parent) = Path::new(&socket_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let _ = std::fs::remove_file(&socket_path);

        let child = std::process::Command::new("mpv")
            .arg("--idle")
            .arg("--no-video")
            .arg(format!("--input-ipc-server={}", socket_path))
            .spawn()
            .ok()?;

        // Connect to socket with retry
        let mut stream = None;
        for _ in 0..20 {
            if let Ok(s) = std::os::unix::net::UnixStream::connect(&socket_path) {
                stream = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let stream = stream?;
        stream.set_nonblocking(true).ok()?;

        Some(Self {
            child,
            stream,
            socket_path,
            incoming_data: Vec::new(),
            elapsed: Duration::from_secs(0),
            duration: None,
            is_paused: false,
            idle_active: true,
            volume: 80,
            ignore_idle: true,
        })
    }

    fn send_command(&mut self, cmd: serde_json::Value) {
        if let Ok(cmd_str) = serde_json::to_string(&cmd) {
            let _ = self.stream.write_all(format!("{}\n", cmd_str).as_bytes());
            let _ = self.stream.flush();
        }
    }

    pub fn play(&mut self, path: &str) -> Result<(), String> {
        let cmd = serde_json::json!({
            "command": ["loadfile", path]
        });
        self.send_command(cmd);
        self.ignore_idle = true;
        self.is_paused = false;
        self.idle_active = false;
        Ok(())
    }

    pub fn pause(&mut self) {
        let cmd = serde_json::json!({
            "command": ["set_property", "pause", true]
        });
        self.send_command(cmd);
        self.is_paused = true;
    }

    pub fn resume(&mut self) {
        let cmd = serde_json::json!({
            "command": ["set_property", "pause", false]
        });
        self.send_command(cmd);
        self.is_paused = false;
    }

    pub fn seek_to(&mut self, target: Duration) {
        let seconds = target.as_secs_f64();
        let cmd = serde_json::json!({
            "command": ["seek", seconds, "absolute"]
        });
        self.send_command(cmd);
        self.elapsed = target;
    }

    pub fn stop(&mut self) {
        let cmd = serde_json::json!({
            "command": ["stop"]
        });
        self.send_command(cmd);
        self.elapsed = Duration::from_secs(0);
        self.is_paused = false;
        self.idle_active = true;
        self.ignore_idle = true;
    }

    pub fn set_volume(&mut self, volume: u8) {
        let cmd = serde_json::json!({
            "command": ["set_property", "volume", volume]
        });
        self.send_command(cmd);
        self.volume = volume;
    }

    pub fn get_volume(&mut self) -> Option<u8> {
        Some(self.volume)
    }

    pub fn get_position(&mut self) -> Duration {
        self.elapsed
    }

    pub fn get_duration(&mut self) -> Option<Duration> {
        self.duration
    }

    pub fn is_finished(&mut self) -> bool {
        if self.ignore_idle {
            false
        } else {
            self.idle_active
        }
    }

    pub fn poll_status(&mut self) {
        // Send queries with request_id
        let q_time = serde_json::json!({
            "command": ["get_property", "time-pos"],
            "request_id": 1
        });
        let q_dur = serde_json::json!({
            "command": ["get_property", "duration"],
            "request_id": 2
        });
        let q_pause = serde_json::json!({
            "command": ["get_property", "pause"],
            "request_id": 3
        });
        let q_idle = serde_json::json!({
            "command": ["get_property", "idle-active"],
            "request_id": 4
        });
        let q_vol = serde_json::json!({
            "command": ["get_property", "volume"],
            "request_id": 5
        });

        self.send_command(q_time);
        self.send_command(q_dur);
        self.send_command(q_pause);
        self.send_command(q_idle);
        self.send_command(q_vol);

        // Read responses
        let mut read_buf = [0u8; 1024];
        loop {
            match self.stream.read(&mut read_buf) {
                Ok(n) if n > 0 => {
                    self.incoming_data.extend_from_slice(&read_buf[..n]);
                }
                Ok(_) => break, // EOF
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(_) => {
                    break;
                }
            }
        }

        // Process line by line
        while let Some(pos) = self.incoming_data.iter().position(|&b| b == b'\n') {
            let line_bytes = self.incoming_data.drain(..=pos).collect::<Vec<u8>>();
            if let Ok(line_str) = std::str::from_utf8(&line_bytes) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(line_str) {
                    if let Some(req_id) = val.get("request_id").and_then(|id| id.as_i64()) {
                        if let Some(data) = val.get("data") {
                            match req_id {
                                1 => {
                                    if let Some(sec) = data.as_f64() {
                                        self.elapsed = Duration::from_secs_f64(sec);
                                    }
                                }
                                2 => {
                                    if let Some(sec) = data.as_f64() {
                                        self.duration = Some(Duration::from_secs_f64(sec));
                                    }
                                }
                                3 => {
                                    if let Some(p) = data.as_bool() {
                                        self.is_paused = p;
                                    }
                                }
                                4 => {
                                    if let Some(idle) = data.as_bool() {
                                        self.idle_active = idle;
                                        if !idle {
                                            self.ignore_idle = false;
                                        }
                                    }
                                }
                                5 => {
                                    if let Some(v) = data.as_f64() {
                                        self.volume = v as u8;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Drop for MpvBackend {
    fn drop(&mut self) {
        let quit_cmd = serde_json::json!({
            "command": ["quit"]
        });
        if let Ok(cmd_str) = serde_json::to_string(&quit_cmd) {
            let _ = self.stream.write_all(format!("{}\n", cmd_str).as_bytes());
            let _ = self.stream.flush();
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
