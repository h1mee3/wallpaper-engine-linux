pub mod mpv_player;
pub mod web_player;

use anyhow::{Context, Result};
pub use mpv_player::ScalingMode;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

use crate::discovery::Wallpaper;
use mpv_player::MpvInstance;
use web_player::WebInstance;

pub enum PlayerBackend {
    Video(MpvInstance),
    Web(WebInstance),
}

impl PlayerBackend {
    pub async fn set_pause(&mut self, paused: bool) -> Result<()> {
        match self {
            PlayerBackend::Video(inst) => inst.set_pause(paused).await,
            PlayerBackend::Web(inst) => inst.set_pause(paused).await,
        }
    }

    pub async fn set_volume(&mut self, volume: u32) -> Result<()> {
        match self {
            PlayerBackend::Video(inst) => inst.set_volume(volume).await,
            PlayerBackend::Web(inst) => inst.set_volume(volume).await,
        }
    }

    pub async fn set_mute(&mut self, mute: bool) -> Result<()> {
        match self {
            PlayerBackend::Video(inst) => inst.set_mute(mute).await,
            PlayerBackend::Web(inst) => inst.set_mute(mute).await,
        }
    }

    pub fn stop(&mut self) {
        match self {
            PlayerBackend::Video(inst) => inst.stop(),
            PlayerBackend::Web(inst) => inst.stop(),
        }
    }
}

pub struct RenderManager {
    backends: HashMap<String, PlayerBackend>,
    current_wallpapers: HashMap<String, Wallpaper>,
    global_paused: bool,
    global_volume: u32,
    global_muted: bool,
    global_scaling: ScalingMode,
}

impl RenderManager {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
            current_wallpapers: HashMap::new(),
            global_paused: false,
            global_volume: 50,
            global_muted: true,
            global_scaling: ScalingMode::Fill,
        }
    }

    /// Apply a wallpaper to a specific monitor with exact display geometry
    pub async fn apply_wallpaper(
        &mut self,
        monitor: &str,
        wallpaper: &Wallpaper,
        width: u32,
        height: u32,
        x: i32,
        y: i32,
    ) -> Result<()> {
        info!(
            "Applying wallpaper '{}' [{:?}] to monitor '{}' ({}x{} at +{}+{})",
            wallpaper.title, wallpaper.wallpaper_type, monitor, width, height, x, y
        );

        let volume = self.global_volume;
        let muted = self.global_muted;
        let paused = self.global_paused;

        if wallpaper.is_web() {
            let web_file = wallpaper.resolve_web_path().with_context(|| {
                format!(
                    "No web entrypoint found for interactive wallpaper '{}' ({})",
                    wallpaper.title, wallpaper.id
                )
            })?;

            let needs_new_backend = match self.backends.get(monitor) {
                Some(PlayerBackend::Web(_)) => false,
                _ => true,
            };

            if needs_new_backend {
                if let Some(mut old) = self.backends.remove(monitor) {
                    old.stop();
                }
                self.backends.insert(
                    monitor.to_string(),
                    PlayerBackend::Web(WebInstance::new(monitor, width, height, x, y)),
                );
            }

            if let Some(PlayerBackend::Web(ref mut inst)) = self.backends.get_mut(monitor) {
                inst.update_geometry(width, height, x, y);
                inst.load_file(&web_file).await?;
                inst.set_volume(volume).await?;
                inst.set_mute(muted).await?;
                if paused {
                    inst.set_pause(true).await?;
                }
            }
        } else {
            // Resolve the best playable media asset (video, animated gif, or high-res image)
            let playable_file = wallpaper.resolve_playable_path().with_context(|| {
                format!(
                    "No playable media found for wallpaper '{}' ({})",
                    wallpaper.title, wallpaper.id
                )
            })?;

            let audio_file = wallpaper.resolve_audio_path();
            let scaling = self.global_scaling;

            let needs_new_backend = match self.backends.get(monitor) {
                Some(PlayerBackend::Video(_)) => false,
                _ => true,
            };

            if needs_new_backend {
                if let Some(mut old) = self.backends.remove(monitor) {
                    old.stop();
                }
                self.backends.insert(
                    monitor.to_string(),
                    PlayerBackend::Video(MpvInstance::new(monitor, width, height, x, y)),
                );
            }

            if let Some(PlayerBackend::Video(ref mut inst)) = self.backends.get_mut(monitor) {
                inst.update_geometry(width, height, x, y);
                inst.set_scaling_mode(scaling).await?;
                inst.set_volume(volume).await?;
                inst.set_mute(muted).await?;
                inst.load_file_with_audio(&playable_file, audio_file.as_deref())
                    .await?;
                if paused {
                    inst.set_pause(true).await?;
                }
            }
        }

        // Synchronize desktop preview wallpaper (Noctalia / Compositor)
        if let Some(ref preview) = wallpaper.preview_path {
            Self::sync_desktop_wallpaper(monitor, preview);
        } else if wallpaper.folder_path.join("preview.jpg").exists() {
            let preview = wallpaper.folder_path.join("preview.jpg");
            Self::sync_desktop_wallpaper(monitor, &preview);
        }

        self.current_wallpapers
            .insert(monitor.to_string(), wallpaper.clone());

        Ok(())
    }

    /// Synchronize wallpaper with the active desktop environment and Noctalia Shell
    fn sync_desktop_wallpaper(monitor: &str, image_path: &Path) {
        crate::compositor::Compositor::sync_wallpaper(monitor, image_path);
        Self::sync_noctalia_wallpaper(monitor, image_path);
    }

    fn sync_noctalia_wallpaper(monitor: &str, image_path: &Path) {
        let p_str = image_path.to_string_lossy().to_string();
        debug!("Syncing Noctalia wallpaper on {}: {}", monitor, p_str);

        let target_arg = if monitor == "*" { "all" } else { monitor };
        let _ = Command::new("noctalia")
            .args(["msg", "wallpaper-set", target_arg, &p_str])
            .output();

        let _ = Command::new("noctalia")
            .args(["msg", "color-scheme-set", "wallpaper", "scheme-tonal-spot"])
            .output();
    }

    /// Set scaling mode across all instances
    pub async fn set_scaling_mode(&mut self, mode: ScalingMode) -> Result<()> {
        self.global_scaling = mode;
        for backend in self.backends.values_mut() {
            if let PlayerBackend::Video(ref mut inst) = backend {
                inst.set_scaling_mode(mode).await?;
            }
        }
        Ok(())
    }

    /// Pause all running wallpaper instances (e.g. when entering fullscreen or covered by apps)
    pub async fn pause_all(&mut self) -> Result<()> {
        self.global_paused = true;
        for (mon, backend) in &mut self.backends {
            debug!("Pausing wallpaper playback on monitor {}", mon);
            backend.set_pause(true).await?;
        }
        Ok(())
    }

    /// Resume all running wallpaper instances
    pub async fn resume_all(&mut self) -> Result<()> {
        self.global_paused = false;
        for (mon, backend) in &mut self.backends {
            debug!("Resuming wallpaper playback on monitor {}", mon);
            backend.set_pause(false).await?;
        }
        Ok(())
    }

    /// Toggle pause / resume state
    pub async fn toggle_all(&mut self) -> Result<bool> {
        if self.global_paused {
            self.resume_all().await?;
            Ok(false)
        } else {
            self.pause_all().await?;
            Ok(true)
        }
    }

    /// Set volume across all instances
    pub async fn set_volume(&mut self, volume: u32) -> Result<()> {
        self.global_volume = volume.min(100);
        for backend in self.backends.values_mut() {
            backend.set_volume(self.global_volume).await?;
        }
        Ok(())
    }

    /// Set mute across all instances
    pub async fn set_mute(&mut self, mute: bool) -> Result<()> {
        self.global_muted = mute;
        for backend in self.backends.values_mut() {
            backend.set_mute(self.global_muted).await?;
        }
        Ok(())
    }

    /// Get current active wallpapers per monitor
    pub fn get_active_wallpapers(&self) -> &HashMap<String, Wallpaper> {
        &self.current_wallpapers
    }

    /// Check if globally paused
    pub fn is_paused(&self) -> bool {
        self.global_paused
    }

    /// Current volume
    pub fn volume(&self) -> u32 {
        self.global_volume
    }

    /// Current mute state
    pub fn is_muted(&self) -> bool {
        self.global_muted
    }

    /// Current scaling mode
    pub fn scaling_mode(&self) -> ScalingMode {
        self.global_scaling
    }

    /// Terminate all instances
    pub fn stop_all(&mut self) {
        for (_, mut backend) in self.backends.drain() {
            backend.stop();
        }
    }
}

impl Drop for RenderManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}
