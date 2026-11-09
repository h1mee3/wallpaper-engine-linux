pub mod mpv_player;

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};


use crate::discovery::{Wallpaper, WallpaperType};
use mpv_player::MpvInstance;

pub struct RenderManager {
    instances: HashMap<String, MpvInstance>,
    current_wallpapers: HashMap<String, Wallpaper>,
    global_paused: bool,
    global_volume: u32,
    global_muted: bool,
}

impl RenderManager {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            current_wallpapers: HashMap::new(),
            global_paused: false,
            global_volume: 50,
            global_muted: true,
        }
    }

    /// Apply a wallpaper to a specific monitor (or "*" for all)
    pub async fn apply_wallpaper(&mut self, monitor: &str, wallpaper: &Wallpaper) -> Result<()> {
        info!(
            "Applying wallpaper '{}' [{:?}] to monitor '{}'",
            wallpaper.title, wallpaper.wallpaper_type, monitor
        );

        match wallpaper.wallpaper_type {
            WallpaperType::Video => {
                let instance = self
                    .instances
                    .entry(monitor.to_string())
                    .or_insert_with(|| MpvInstance::new(monitor));

                instance.set_volume(self.global_volume).await?;
                instance.set_mute(self.global_muted).await?;
                instance.load_file(&wallpaper.full_file_path).await?;

                // Also update Noctalia's wallpaper to the preview image so lockscreen/desktop theme colors sync
                if let Some(ref preview) = wallpaper.preview_path {
                    Self::sync_noctalia_wallpaper(monitor, preview);
                }
            }
            WallpaperType::Scene | WallpaperType::Web | _ => {
                // If there's an active video instance on this monitor, stop it
                if let Some(mut inst) = self.instances.remove(monitor) {
                    inst.stop();
                }

                // If preview or extracted image exists, set via Noctalia / swaybg
                if let Some(ref preview) = wallpaper.preview_path {
                    Self::sync_noctalia_wallpaper(monitor, preview);
                } else if wallpaper.folder_path.join("preview.jpg").exists() {
                    let preview = wallpaper.folder_path.join("preview.jpg");
                    Self::sync_noctalia_wallpaper(monitor, &preview);
                }
            }
        }

        self.current_wallpapers
            .insert(monitor.to_string(), wallpaper.clone());

        Ok(())
    }

    /// Notify Noctalia about the new wallpaper so its color scheme, theme, and greeter sync
    fn sync_noctalia_wallpaper(monitor: &str, image_path: &Path) {
        let p_str = image_path.to_string_lossy().to_string();
        debug!("Syncing Noctalia wallpaper on {}: {}", monitor, p_str);

        let target_arg = if monitor == "*" { "all" } else { monitor };
        let _ = Command::new("noctalia")
            .args(["msg", "wallpaper-set", target_arg, &p_str])
            .output();

        // Optionally trigger theme generation
        let _ = Command::new("noctalia")
            .args(["msg", "color-scheme-set", "wallpaper", "scheme-tonal-spot"])
            .output();
    }

    /// Pause all running wallpaper instances (e.g. when entering fullscreen in a game)
    pub async fn pause_all(&mut self) -> Result<()> {
        self.global_paused = true;
        for (mon, inst) in &mut self.instances {
            debug!("Pausing wallpaper playback on monitor {}", mon);
            inst.set_pause(true).await?;
        }
        Ok(())
    }

    /// Resume all running wallpaper instances
    pub async fn resume_all(&mut self) -> Result<()> {
        self.global_paused = false;
        for (mon, inst) in &mut self.instances {
            debug!("Resuming wallpaper playback on monitor {}", mon);
            inst.set_pause(false).await?;
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
        for inst in self.instances.values_mut() {
            inst.set_volume(self.global_volume).await?;
        }
        Ok(())
    }

    /// Set mute across all instances
    pub async fn set_mute(&mut self, mute: bool) -> Result<()> {
        self.global_muted = mute;
        for inst in self.instances.values_mut() {
            inst.set_mute(self.global_muted).await?;
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

    /// Terminate all instances
    pub fn stop_all(&mut self) {
        for (_, mut inst) in self.instances.drain() {
            inst.stop();
        }
    }
}

impl Drop for RenderManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}
