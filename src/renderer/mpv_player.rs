use anyhow::{Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, info, warn};


pub struct MpvInstance {
    pub monitor: String,
    pub socket_path: PathBuf,
    pub process: Option<Child>,
    pub current_file: Option<PathBuf>,
    pub paused: bool,
    pub volume: u32,
    pub muted: bool,
}

impl MpvInstance {
    pub fn new(monitor: &str) -> Self {
        let socket_path = PathBuf::from(format!("/tmp/wengine-mpv-{}.sock", monitor));
        Self {
            monitor: monitor.to_string(),
            socket_path,
            process: None,
            current_file: None,
            paused: false,
            volume: 0, // default muted for background wallpaper
            muted: true,
        }
    }

    /// Ensure MPV background process is running with IPC socket
    pub fn ensure_running(&mut self) -> Result<()> {
        if self.is_alive() {
            return Ok(());
        }

        // Clean up stale socket if present
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        info!("Spawning MPV wallpaper backend for monitor {}", self.monitor);

        let app_id = format!("wengine-bg-{}", self.monitor);
        let socket_arg = format!("--input-ipc-server={}", self.socket_path.to_string_lossy());
        let app_id_arg = format!("--wayland-app-id={}", app_id);
        let title_arg = format!("--title={}", app_id);
        let volume_arg = format!("--volume={}", self.volume);

        let mut cmd = Command::new("mpv");
        cmd.args([
            "--no-config",
            "--idle=yes",
            "--loop-file=inf",
            "--hwdec=auto-safe",
            "--no-audio", // initial mute
            "--no-osc",
            "--no-osd-bar",
            "--no-input-default-bindings",
            "--no-stop-screensaver",
            "--really-quiet",
            &socket_arg,
            &app_id_arg,
            &title_arg,
            &volume_arg,
        ])

        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        let child = cmd.spawn().context("Failed to spawn mpv process")?;
        self.process = Some(child);

        // Wait up to 1.5s for the IPC socket to appear
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if self.socket_path.exists() {
                debug!("MPV IPC socket ready at {:?}", self.socket_path);
                return Ok(());
            }
        }

        warn!("MPV process spawned but socket {:?} took long to create", self.socket_path);
        Ok(())
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

    /// Send a JSON command to the MPV IPC socket
    pub async fn send_command(&self, cmd: serde_json::Value) -> Result<serde_json::Value> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| format!("Failed to connect to MPV socket at {:?}", self.socket_path))?;

        let mut payload = serde_json::to_vec(&cmd)?;
        payload.push(b'\n');
        stream.write_all(&payload).await?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let resp: serde_json::Value = serde_json::from_str(&line)?;
        Ok(resp)
    }

    /// Load a video file seamlessly (gapless replace)
    pub async fn load_file(&mut self, path: &Path) -> Result<()> {
        self.ensure_running()?;

        let path_str = path.to_string_lossy().to_string();
        info!("Loading video on monitor {}: {}", self.monitor, path_str);

        let cmd = json!({
            "command": ["loadfile", path_str, "replace"]
        });

        self.send_command(cmd).await?;
        self.current_file = Some(path.to_path_buf());
        self.paused = false;

        // Apply audio settings
        self.apply_audio().await?;

        Ok(())
    }

    /// Pause or resume playback
    pub async fn set_pause(&mut self, pause: bool) -> Result<()> {
        if !self.socket_path.exists() {
            return Ok(());
        }

        let cmd = json!({
            "command": ["set_property", "pause", pause]
        });

        self.send_command(cmd).await?;
        self.paused = pause;
        debug!("Monitor {} pause set to {}", self.monitor, pause);
        Ok(())
    }

    /// Set volume (0..100)
    pub async fn set_volume(&mut self, volume: u32) -> Result<()> {
        self.volume = volume.min(100);
        self.apply_audio().await?;
        Ok(())
    }

    /// Set mute
    pub async fn set_mute(&mut self, mute: bool) -> Result<()> {
        self.muted = mute;
        self.apply_audio().await?;
        Ok(())
    }

    async fn apply_audio(&self) -> Result<()> {
        if !self.socket_path.exists() {
            return Ok(());
        }

        let mute_cmd = json!({
            "command": ["set_property", "mute", self.muted]
        });
        let _ = self.send_command(mute_cmd).await;

        let vol_cmd = json!({
            "command": ["set_property", "volume", self.volume]
        });
        let _ = self.send_command(vol_cmd).await;

        Ok(())
    }

    /// Stop and terminate the instance
    pub fn stop(&mut self) {
        if let Some(ref mut child) = self.process {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.process = None;
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

impl Drop for MpvInstance {
    fn drop(&mut self) {
        self.stop();
    }
}
