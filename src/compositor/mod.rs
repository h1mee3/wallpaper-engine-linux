pub mod detection;
pub mod drm;
pub mod events;
pub mod kde;
pub mod monitor;
pub mod sway;
pub mod wallpaper_sync;
pub mod wayland;

use anyhow::Result;
use tracing::{debug, info, warn};

pub use detection::{detect_desktop_environment, DesktopEnvironment};
pub use events::{CompositorEvent, CompositorEvents};
pub use kde::Kde;
pub use monitor::Monitor;
pub use wallpaper_sync::WallpaperSync;

use crate::hyprland::Hyprland;

pub struct Compositor;

impl Compositor {
    /// Detect the active desktop environment / window manager
    pub fn current_de() -> DesktopEnvironment {
        detect_desktop_environment()
    }

    /// Query all active displays using a multi-tiered discovery strategy:
    /// 1. Native Wayland client protocol (`wl_output` via `wayland-client`)
    /// 2. Desktop-specific CLI tools (`hyprctl`, `swaymsg`, `wlr-randr`, `kscreen-doctor`)
    /// 3. Linux Kernel DRM subsystem (`/sys/class/drm`)
    /// 4. Safe fallback (1920x1080 @ 60Hz)
    pub fn get_monitors() -> Result<Vec<Monitor>> {
        // Tier 1: Direct native Wayland client protocol
        if let Ok(mons) = wayland::query_wayland_monitors() {
            if !mons.is_empty() {
                debug!("Discovered {} monitors via native Wayland protocol", mons.len());
                return Ok(mons);
            }
        }

        let de = detect_desktop_environment();
        debug!("Wayland protocol query returned 0 monitors; falling back to DE-specific query for {}", de.name());

        // Tier 2: Compositor CLI / IPC
        match de {
            DesktopEnvironment::Hyprland => {
                if let Ok(mons) = Hyprland::get_monitors() {
                    if !mons.is_empty() {
                        return Ok(mons);
                    }
                }
            }
            DesktopEnvironment::Sway => {
                if let Ok(mons) = sway::Sway::get_monitors() {
                    if !mons.is_empty() {
                        return Ok(mons);
                    }
                }
            }
            DesktopEnvironment::KdePlasma => {
                if let Ok(mons) = Kde::get_monitors() {
                    if !mons.is_empty() {
                        return Ok(mons);
                    }
                }
            }
            _ => {}
        }

        // Try kscreen-doctor anyway if available
        if let Ok(mons) = Kde::get_monitors() {
            if !mons.is_empty() {
                return Ok(mons);
            }
        }

        // Try swaymsg anyway if available
        if let Ok(mons) = sway::Sway::get_monitors() {
            if !mons.is_empty() {
                return Ok(mons);
            }
        }

        // Try hyprctl anyway if available
        if let Ok(mons) = Hyprland::get_monitors() {
            if !mons.is_empty() {
                return Ok(mons);
            }
        }

        // Tier 3: Linux Kernel DRM subsystem fallback
        let drm_mons = drm::query_drm_monitors();
        if !drm_mons.is_empty() {
            info!("Discovered {} monitors via /sys/class/drm", drm_mons.len());
            return Ok(drm_mons);
        }

        // Tier 4: Fallback single display
        warn!("Could not detect monitors; using fallback display (1920x1080)");
        Ok(vec![Monitor {
            id: 0,
            name: "DP-1".to_string(),
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            scale: 1.0,
            refresh_rate: 60.0,
            focused: true,
        }])
    }

    /// Set up any necessary compositor window or layer rules
    pub fn setup_compositor_rules() {
        let de = detect_desktop_environment();
        info!("Setting up compositor rules for {}", de.name());

        if de == DesktopEnvironment::Hyprland {
            Hyprland::setup_window_rules();
        }
    }

    /// Start listening for compositor events (fullscreen auto-pause, monitor hotplug)
    pub fn start_event_listener(
        tx: tokio::sync::mpsc::UnboundedSender<CompositorEvent>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        CompositorEvents::start_listener(tx)
    }

    /// Synchronize wallpaper with the active desktop environment
    pub fn sync_wallpaper(monitor: &str, image_path: &std::path::Path) {
        WallpaperSync::sync_desktop(monitor, image_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compositor_get_monitors() {
        let mons = Compositor::get_monitors();
        assert!(mons.is_ok());
        let list = mons.unwrap();
        println!("Compositor monitors: {:?}", list);
        assert!(!list.is_empty());
    }
}

