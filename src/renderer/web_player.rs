use anyhow::{Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, info};

pub struct WebInstance {
    pub monitor: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub socket_path: PathBuf,
    pub process: Option<Child>,
    pub current_file: Option<PathBuf>,
    pub paused: bool,
    pub volume: u32,
    pub muted: bool,
}

impl WebInstance {
    pub fn new(monitor: &str, width: u32, height: u32, x: i32, y: i32) -> Self {
        let socket_path = PathBuf::from(format!("/tmp/wengine-web-{}.sock", monitor));
        Self {
            monitor: monitor.to_string(),
            width,
            height,
            x,
            y,
            socket_path,
            process: None,
            current_file: None,
            paused: false,
            volume: 100,
            muted: false,
        }
    }

    pub fn update_geometry(&mut self, width: u32, height: u32, x: i32, y: i32) {
        self.width = width;
        self.height = height;
        self.x = x;
        self.y = y;
    }

    pub fn find_web_player_bin() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let p = parent.join("wengine-web-player");
                if p.exists() {
                    return p;
                }
            }
        }
        if let Some(home) = dirs::home_dir() {
            let p = home.join(".local/bin/wengine-web-player");
            if p.exists() {
                return p;
            }
        }
        PathBuf::from("wengine-web-player")
    }

    pub fn is_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.process {
            match child.try_wait() {
                Ok(None) => true,
                _ => {
                    self.process = None;
                    false
                }
            }
        } else {
            false
        }
    }

    /// Ensure the native WebKitGTK Layer Shell player is running
    pub fn ensure_running(&mut self, file: &Path) -> Result<()> {
        if self.is_alive() {
            return Ok(());
        }

        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        info!(
            "Spawning native WebKitGTK Layer Shell wallpaper backend for monitor {} (file: {:?})",
            self.monitor, file
        );

        let bin_path = Self::find_web_player_bin();
        let socket_str = self.socket_path.to_string_lossy().to_string();
        let file_arg = file.to_string_lossy().to_string();

        let mut cmd = Command::new(bin_path);
        cmd.args([
            &file_arg,
            &self.monitor,
            &socket_str,
            "bottom",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        let child = cmd.spawn().context("Failed to spawn wengine-web-player backend")?;
        self.process = Some(child);

        // Wait up to 2.5s for IPC socket to become available
        for _ in 0..50 {
            if self.socket_path.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        Ok(())
    }

    /// Send an IPC command to the web player
    pub async fn send_command(&self, cmd: &serde_json::Value) -> Result<serde_json::Value> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| format!("Failed to connect to web player socket {:?}", self.socket_path))?;

        let mut payload = serde_json::to_vec(cmd)?;
        payload.push(b'\n');
        stream.write_all(&payload).await?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let resp: serde_json::Value = serde_json::from_str(&line).unwrap_or(json!({"status": "ok"}));
        Ok(resp)
    }

    /// Load or switch HTML file in the web player
    pub async fn load_file(&mut self, path: &Path) -> Result<()> {
        if !self.is_alive() || self.current_file.as_deref() != Some(path) {
            self.stop();
            self.ensure_running(path)?;
            self.current_file = Some(path.to_path_buf());
        }
        Ok(())
    }

    /// Set pause / resume
    pub async fn set_pause(&mut self, pause: bool) -> Result<()> {
        self.paused = pause;
        if self.is_alive() {
            debug!("Sending pause={} to web player on {}", pause, self.monitor);
            let cmd = if pause {
                json!({"command": "pause"})
            } else {
                json!({"command": "resume"})
            };
            let _ = self.send_command(&cmd).await;
        }
        Ok(())
    }

    /// Set volume
    pub async fn set_volume(&mut self, volume: u32) -> Result<()> {
        self.volume = volume;
        if self.is_alive() {
            let cmd = json!({"command": "volume", "value": volume});
            let _ = self.send_command(&cmd).await;
        }
        Ok(())
    }

    /// Set mute
    pub async fn set_mute(&mut self, mute: bool) -> Result<()> {
        self.muted = mute;
        if self.is_alive() {
            let cmd = json!({"command": "mute", "value": mute});
            let _ = self.send_command(&cmd).await;
        }
        Ok(())
    }

    /// Terminate web player
    pub fn stop(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        self.current_file = None;
    }
}

impl Drop for WebInstance {
    fn drop(&mut self) {
        self.stop();
    }
}
