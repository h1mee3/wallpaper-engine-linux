use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

use super::detection::{detect_desktop_environment, DesktopEnvironment};

pub struct WallpaperSync;

impl WallpaperSync {
    /// Synchronize static or preview wallpaper image with the active desktop environment
    pub fn sync_desktop(monitor: &str, image_path: &Path) {
        let path_str = image_path.to_string_lossy().to_string();
        let de = detect_desktop_environment();

        debug!(
            "Syncing desktop wallpaper for monitor '{}' to '{}' on DE: {}",
            monitor, path_str, de.name()
        );

        match de {
            DesktopEnvironment::Gnome => {
                let uri = format!("file://{}", path_str);
                let _ = Command::new("gsettings")
                    .args(["set", "org.gnome.desktop.background", "picture-uri", &uri])
                    .output();
                let _ = Command::new("gsettings")
                    .args(["set", "org.gnome.desktop.background", "picture-uri-dark", &uri])
                    .output();
                info!("Synced wallpaper with GNOME background via gsettings");
            }
            DesktopEnvironment::KdePlasma => {
                // KDE Plasma allows setting wallpaper via dbus evaluateScript
                let script = format!(
                    r#"for (let d of desktops()) {{ d.wallpaperPlugin = "org.kde.image"; d.currentConfigGroup = Array("Wallpaper", "org.kde.image", "General"); d.writeConfig("Image", "file://{}"); }}"#,
                    path_str
                );
                let _ = Command::new("qdbus")
                    .args([
                        "org.kde.plasmashell",
                        "/PlasmaShell",
                        "org.kde.PlasmaShell.evaluateScript",
                        &script,
                    ])
                    .output();
                info!("Synced wallpaper with KDE Plasma via DBus");
            }
            DesktopEnvironment::Sway => {
                let mon_arg = if monitor == "*" { "*" } else { monitor };
                let _ = Command::new("swaymsg")
                    .args(["output", mon_arg, "bg", &path_str, "fill"])
                    .output();
                info!("Synced wallpaper with Sway via swaymsg");
            }
            DesktopEnvironment::Hyprland => {
                // If hyprpaper is running, sync to it
                let _ = Command::new("hyprctl")
                    .args([
                        "hyprpaper",
                        "wallpaper",
                        &format!("{},{}", monitor, path_str),
                    ])
                    .output();
            }
            _ => {}
        }

        // Also sync swww if swww-daemon is running
        let _ = Command::new("swww")
            .args(["img", &path_str, "--transition-type", "wipe"])
            .output();

        // Also check if Noctalia integration applies
        Self::sync_noctalia(monitor, image_path);
    }

    /// Sync with Noctalia Shell if installed
    pub fn sync_noctalia(_monitor: &str, image_path: &Path) {
        if let Some(home) = dirs::home_dir() {
            let noc_bin = home.join(".local/bin/noctalia");
            if noc_bin.exists() {
                let _ = Command::new(noc_bin)
                    .args(["wallpaper", "set", image_path.to_str().unwrap_or_default()])
                    .output();
                return;
            }
        }

        let _ = Command::new("noctalia")
            .args(["wallpaper", "set", image_path.to_str().unwrap_or_default()])
            .output();
    }
}
