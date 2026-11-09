use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScalingMode {
    Fill,    // Cover / Crop to fill 100% of display (zero black bars)
    Fit,     // Letterbox / Pillarbox
    Stretch, // Stretch to exact display aspect ratio
}

impl Default for ScalingMode {
    fn default() -> Self {
        ScalingMode::Fill
    }
}

pub struct MpvInstance {
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
    pub scaling_mode: ScalingMode,
}

impl MpvInstance {
    pub fn new(monitor: &str, width: u32, height: u32, x: i32, y: i32) -> Self {
        let socket_path = PathBuf::from(format!("/tmp/wengine-mpv-{}.sock", monitor));
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
            volume: 0, // default muted for background wallpaper
            muted: true,
            scaling_mode: ScalingMode::Fill,
        }
    }

    /// Update target display geometry
    pub fn update_geometry(&mut self, width: u32, height: u32, x: i32, y: i32) {
        self.width = width;
        self.height = height;
        self.x = x;
        self.y = y;
    }

    pub fn find_layer_player_bin() -> PathBuf {
        if let Some(home) = dirs::home_dir() {
            let p = home.join(".local/bin/wengine-layer-player");
            if p.exists() {
                return p;
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let p = parent.join("wengine-layer-player");
                if p.exists() {
                    return p;
                }
            }
        }
        PathBuf::from("wengine-layer-player")
    }

    /// Ensure native Wayland Layer Shell backend is running with IPC socket and exact display resolution
    pub fn ensure_running(&mut self, initial_file: Option<&Path>) -> Result<()> {
        if self.is_alive() {
            return Ok(());
        }

        // Clean up stale socket if present
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        info!(
            "Spawning native Wayland Layer Shell wallpaper backend for monitor {} (resolution: {}x{} at +{}+{})",
            self.monitor, self.width, self.height, self.x, self.y
        );

        let bin_path = Self::find_layer_player_bin();
        let socket_str = self.socket_path.to_string_lossy().to_string();
        let file_arg = initial_file
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut cmd = Command::new(bin_path);
        cmd.args([
            &file_arg,
            &self.monitor,
            &socket_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        let child = cmd.spawn().context("Failed to spawn wengine-layer-player backend")?;
        self.process = Some(child);

        // Wait up to 2.5s for the IPC socket to appear
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if self.socket_path.exists() {
                debug!("Layer player IPC socket ready at {:?}", self.socket_path);
                return Ok(());
            }
        }

        warn!(
            "Layer player spawned but socket {:?} took long to create",
            self.socket_path
        );
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

    /// Load a media file (video, animated gif, or image) with optional accompanying audio track
    pub async fn load_file_with_audio(
        &mut self,
        path: &Path,
        audio_path: Option<&Path>,
    ) -> Result<()> {
        let is_running = self.is_alive();
        if !is_running {
            self.ensure_running(Some(path))?;
        }

        let path_str = path.to_string_lossy().to_string();
        info!("Loading media on monitor {}: {}", self.monitor, path_str);

        if is_running {
            let cmd = json!({
                "command": ["loadfile", path_str, "replace"]
            });
            self.send_command(cmd).await?;
        }

        // If an external audio track is provided, attach it
        if let Some(audio) = audio_path {
            let audio_str = audio.to_string_lossy().to_string();
            info!("Attaching audio track on monitor {}: {}", self.monitor, audio_str);
            let audio_cmd = json!({
                "command": ["audio-add", audio_str, "auto"]
            });
            let _ = self.send_command(audio_cmd).await;
        }

        self.current_file = Some(path.to_path_buf());
        self.paused = false;

        // Apply audio & scaling settings
        let _ = self.apply_audio().await;
        let _ = self.apply_scaling().await;

        Ok(())
    }

    /// Load a media file seamlessly (gapless replace)
    pub async fn load_file(&mut self, path: &Path) -> Result<()> {
        self.load_file_with_audio(path, None).await
    }

    /// Change scaling mode in real time
    pub async fn set_scaling_mode(&mut self, mode: ScalingMode) -> Result<()> {
        self.scaling_mode = mode;
        self.apply_scaling().await?;
        Ok(())
    }

    async fn apply_scaling(&self) -> Result<()> {
        if !self.socket_path.exists() {
            return Ok(());
        }

        let panscan = match self.scaling_mode {
            ScalingMode::Fill => 1.0,
            _ => 0.0,
        };
        let keepaspect = self.scaling_mode != ScalingMode::Stretch;

        let _ = self
            .send_command(json!({
                "command": ["set_property", "panscan", panscan]
            }))
            .await;

        let _ = self
            .send_command(json!({
                "command": ["set_property", "keepaspect", keepaspect]
            }))
            .await;

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
