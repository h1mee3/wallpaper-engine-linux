use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Command;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::{debug, info};

use super::events::CompositorEvent;
use super::monitor::Monitor;

pub struct Kde;

impl Kde {
    pub const PLUGIN_ID: &'static str = "org.kde.plasma.wengine";

    /// Ensure the native Plasma wallpaper plugin is installed into ~/.local/share/plasma/wallpapers/
    pub fn ensure_wallpaper_plugin() -> Result<()> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        let base_dir = home.join(".local/share/plasma/wallpapers").join(Self::PLUGIN_ID);
        let config_dir = base_dir.join("contents/config");
        let ui_dir = base_dir.join("contents/ui");

        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&ui_dir)?;

        let metadata_path = base_dir.join("metadata.json");
        let xml_path = config_dir.join("main.xml");
        let qml_path = ui_dir.join("main.qml");

        const METADATA_JSON: &str = include_str!("../../assets/plasma-wallpaper/org.kde.plasma.wengine/metadata.json");
        const MAIN_XML: &str = include_str!("../../assets/plasma-wallpaper/org.kde.plasma.wengine/contents/config/main.xml");
        const MAIN_QML: &str = include_str!("../../assets/plasma-wallpaper/org.kde.plasma.wengine/contents/ui/main.qml");

        if !metadata_path.exists() || std::fs::read_to_string(&metadata_path).unwrap_or_default() != METADATA_JSON {
            std::fs::write(&metadata_path, METADATA_JSON)?;
        }

        if !xml_path.exists() || std::fs::read_to_string(&xml_path).unwrap_or_default() != MAIN_XML {
            std::fs::write(&xml_path, MAIN_XML)?;
        }

        if !qml_path.exists() || std::fs::read_to_string(&qml_path).unwrap_or_default() != MAIN_QML {
            std::fs::write(&qml_path, MAIN_QML)?;
        }

        Ok(())
    }

    /// Execute a JavaScript snippet in plasmashell via DBus evaluateScript
    pub fn evaluate_script(script: &str) -> Result<String> {
        let mut last_err = String::new();
        for attempt in 0..8 {
            let output = Command::new("qdbus")
                .args(["org.kde.plasmashell", "/PlasmaShell", "org.kde.PlasmaShell.evaluateScript", script])
                .output()
                .context("Failed to run qdbus evaluateScript")?;

            if output.status.success() {
                return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
            }

            last_err = String::from_utf8_lossy(&output.stderr).to_string();
            // Retry briefly if plasmashell is still registering on D-Bus during session startup
            if attempt < 7 && (last_err.contains("does not exist") || last_err.contains("ServiceUnknown") || last_err.contains("Could not connect")) {
                std::thread::sleep(std::time::Duration::from_millis(300));
                continue;
            }
            break;
        }

        anyhow::bail!("evaluateScript failed: {}", last_err);
    }

    /// Apply live video or static wallpaper on KDE Plasma desktops
    pub fn apply_wallpaper(
        monitor: &str,
        video_path: Option<&std::path::Path>,
        image_path: Option<&std::path::Path>,
        scaling_mode: &str,
        muted: bool,
        volume: u32,
    ) -> Result<()> {
        Self::ensure_wallpaper_plugin()?;

        let video_str = video_path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let image_str = image_path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default();

        let video_js = serde_json::to_string(&video_str)?;
        let image_js = serde_json::to_string(&image_str)?;
        let scaling_js = serde_json::to_string(scaling_mode)?;

        let screen_match = if monitor == "*" || monitor.is_empty() {
            "true".to_string()
        } else if let Ok(idx) = monitor.parse::<i32>() {
            format!("d[i].screen === {}", idx)
        } else if let Ok(mons) = Self::get_monitors() {
            if let Some(pos) = mons.iter().position(|m| m.name == monitor) {
                format!("d[i].screen === {}", pos)
            } else {
                "true".to_string()
            }
        } else {
            "true".to_string()
        };

        let script = format!(
            r#"var d = desktops();
for (var i = 0; i < d.length; i++) {{
    if ({screen_match}) {{
        d[i].wallpaperPlugin = "{plugin}";
        d[i].currentConfigGroup = ["Wallpaper", "{plugin}", "General"];
        d[i].writeConfig("VideoPath", {video_js});
        d[i].writeConfig("ImagePath", {image_js});
        d[i].writeConfig("ScalingMode", {scaling_js});
        d[i].writeConfig("Muted", {muted});
        d[i].writeConfig("Volume", {volume});
        d[i].writeConfig("Paused", false);
        d[i].reloadConfig();
    }}
}}"#,
            screen_match = screen_match,
            plugin = Self::PLUGIN_ID,
            video_js = video_js,
            image_js = image_js,
            scaling_js = scaling_js,
            muted = muted,
            volume = volume,
        );

        Self::evaluate_script(&script)?;
        info!("Applied wallpaper to KDE Plasma desktops (monitor: '{}', video: '{}', image: '{}')", monitor, video_str, image_str);
        Ok(())
    }

    /// Pause or resume playback on KDE Plasma
    pub fn set_paused(paused: bool) -> Result<()> {
        let script = format!(
            r#"var d = desktops();
for (var i = 0; i < d.length; i++) {{
    if (d[i].wallpaperPlugin === "{plugin}") {{
        d[i].currentConfigGroup = ["Wallpaper", "{plugin}", "General"];
        d[i].writeConfig("Paused", {paused});
        d[i].reloadConfig();
    }}
}}"#,
            plugin = Self::PLUGIN_ID,
            paused = paused
        );
        Self::evaluate_script(&script)?;
        debug!("KDE Plasma wallpaper paused set to {}", paused);
        Ok(())
    }

    /// Update audio settings on KDE Plasma
    pub fn set_audio(muted: bool, volume: u32) -> Result<()> {
        let script = format!(
            r#"var d = desktops();
for (var i = 0; i < d.length; i++) {{
    if (d[i].wallpaperPlugin === "{plugin}") {{
        d[i].currentConfigGroup = ["Wallpaper", "{plugin}", "General"];
        d[i].writeConfig("Muted", {muted});
        d[i].writeConfig("Volume", {volume});
        d[i].reloadConfig();
    }}
}}"#,
            plugin = Self::PLUGIN_ID,
            muted = muted,
            volume = volume
        );
        Self::evaluate_script(&script)?;
        debug!("KDE Plasma wallpaper audio set: muted={}, volume={}", muted, volume);
        Ok(())
    }

    /// Update scaling mode on KDE Plasma
    pub fn set_scaling(scaling: &str) -> Result<()> {
        let scaling_js = serde_json::to_string(scaling)?;
        let script = format!(
            r#"var d = desktops();
for (var i = 0; i < d.length; i++) {{
    if (d[i].wallpaperPlugin === "{plugin}") {{
        d[i].currentConfigGroup = ["Wallpaper", "{plugin}", "General"];
        d[i].writeConfig("ScalingMode", {scaling_js});
        d[i].reloadConfig();
    }}
}}"#,
            plugin = Self::PLUGIN_ID,
            scaling_js = scaling_js
        );
        Self::evaluate_script(&script)?;
        debug!("KDE Plasma wallpaper scaling set to {}", scaling);
        Ok(())
    }

    /// Query monitors via `kscreen-doctor -j`
    pub fn get_monitors() -> Result<Vec<Monitor>> {
        let output = Command::new("kscreen-doctor")
            .arg("-j")
            .output()
            .context("Failed to execute kscreen-doctor -j")?;

        if !output.status.success() {
            anyhow::bail!(
                "kscreen-doctor failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let json: Value = serde_json::from_slice(&output.stdout)?;
        let outputs = json["outputs"]
            .as_array()
            .context("Expected 'outputs' array in kscreen-doctor output")?;

        let mut monitors = Vec::new();
        for (idx, out) in outputs.iter().enumerate() {
            let enabled = out["enabled"].as_bool().unwrap_or(true);
            let connected = out["connected"].as_bool().unwrap_or(true);
            if !enabled || !connected {
                continue;
            }

            let name = out["name"].as_str().unwrap_or("?").to_string();
            let pos = &out["pos"];
            let x = pos["x"].as_i64().unwrap_or(0) as i32;
            let y = pos["y"].as_i64().unwrap_or(0) as i32;

            let mut width = out["size"]["width"].as_u64().unwrap_or(1920) as u32;
            let mut height = out["size"]["height"].as_u64().unwrap_or(1080) as u32;
            let scale = out["scale"].as_f64().unwrap_or(1.0);
            let mut refresh_rate = 60.0;

            let cur_mode_id = out["currentModeId"].as_str().unwrap_or_default();
            if let Some(modes) = out["modes"].as_array() {
                for mode in modes {
                    let mode_id = mode["id"].as_str().unwrap_or_default();
                    if mode_id == cur_mode_id {
                        if let Some(w) = mode["size"]["width"].as_u64() {
                            width = w as u32;
                        }
                        if let Some(h) = mode["size"]["height"].as_u64() {
                            height = h as u32;
                        }
                        if let Some(rr) = mode["refreshRate"].as_f64() {
                            refresh_rate = rr;
                        }
                        break;
                    }
                }
            }

            monitors.push(Monitor {
                id: idx as u32,
                name,
                width,
                height,
                x,
                y,
                scale,
                refresh_rate,
                focused: idx == 0,
            });
        }

        if monitors.is_empty() {
            anyhow::bail!("No active outputs returned by kscreen-doctor");
        }

        Ok(monitors)
    }

    /// Check if any active application window is fullscreen on KDE Plasma / KWin
    pub fn is_fullscreen_active() -> bool {
        // 1. If showingDesktop is true, all windows are minimized/hidden
        if let Ok(output) = Command::new("qdbus")
            .args(["org.kde.KWin", "/KWin", "org.kde.KWin.showingDesktop"])
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_lowercase();
                if s == "true" {
                    return false;
                }
            }
        }

        // 2. Query open windows via WindowsRunner Match
        if let Ok(output) = Command::new("qdbus")
            .args(["--literal", "org.kde.KWin", "/WindowsRunner", "org.kde.krunner1.Match", ""])
            .output()
        {
            if output.status.success() {
                let out_str = String::from_utf8_lossy(&output.stdout);
                // Look for UUID patterns {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}
                for cap in out_str.split('{') {
                    if let Some(end) = cap.find('}') {
                        let uuid = format!("{{{}}}", &cap[..end]);
                        if uuid.len() == 38 {
                            if let Ok(win_info) = Command::new("qdbus")
                                .args(["org.kde.KWin", "/KWin", "org.kde.KWin.getWindowInfo", &uuid])
                                .output()
                            {
                                if win_info.status.success() {
                                    let info_text = String::from_utf8_lossy(&win_info.stdout);
                                    let is_fs = info_text.contains("fullscreen: true");
                                    let is_min = info_text.contains("minimized: true");
                                    if is_fs && !is_min {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        false
    }

    /// Check whether any active window is fullscreen or covering the desktop on KDE Plasma / KWin.
    /// Returns `(has_fullscreen, has_apps_above)`.
    pub fn check_apps_above() -> (bool, bool) {
        let mut has_fullscreen = false;
        let mut has_apps_above = false;

        if let Ok(output) = Command::new("qdbus")
            .args(["--literal", "org.kde.KWin", "/WindowsRunner", "org.kde.krunner1.Match", ""])
            .output()
        {
            if output.status.success() {
                let out_str = String::from_utf8_lossy(&output.stdout);
                for cap in out_str.split('{') {
                    if let Some(end) = cap.find('}') {
                        let uuid = format!("{{{}}}", &cap[..end]);
                        if uuid.len() == 38 {
                            if let Ok(win_info) = Command::new("qdbus")
                                .args(["org.kde.KWin", "/KWin", "org.kde.KWin.getWindowInfo", &uuid])
                                .output()
                            {
                                if win_info.status.success() {
                                    let info_text = String::from_utf8_lossy(&win_info.stdout);
                                    let is_min = info_text.contains("minimized: true");
                                    if !is_min {
                                        has_apps_above = true;
                                        if info_text.contains("fullscreen: true") {
                                            has_fullscreen = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (has_fullscreen, has_apps_above)
    }

    /// Start listening to KDE Plasma / KWin events (fullscreen auto-pause, display changes)
    pub fn start_event_listener(tx: mpsc::UnboundedSender<CompositorEvent>) -> Option<tokio::task::JoinHandle<()>> {
        info!("Starting KDE Plasma / KWin event listener");
        Some(tokio::spawn(async move {
            let mut last_fullscreen = false;
            let mut last_monitors_count = 0;

            if let Ok(mons) = Self::get_monitors() {
                last_monitors_count = mons.len();
            }

            let mut tick: u64 = 0;
            loop {
                sleep(Duration::from_millis(400)).await;
                tick = tick.wrapping_add(1);

                // Check fullscreen state
                let current_fullscreen = Self::is_fullscreen_active();
                if current_fullscreen != last_fullscreen {
                    last_fullscreen = current_fullscreen;
                    debug!("KDE Plasma fullscreen state changed: {}", current_fullscreen);
                    if tx.send(CompositorEvent::Fullscreen(current_fullscreen)).is_err() {
                        break;
                    }
                }

                // Check monitor hotplug every 4 seconds (~10 ticks)
                if tick % 10 == 0 {
                    if let Ok(mons) = Self::get_monitors() {
                        if mons.len() != last_monitors_count {
                            info!("KDE Plasma monitor count changed: {} -> {}", last_monitors_count, mons.len());
                            last_monitors_count = mons.len();
                            let first_name = mons.first().map(|m| m.name.clone()).unwrap_or_else(|| "output".to_string());
                            let _ = tx.send(CompositorEvent::MonitorAdded(first_name));
                        }
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kde_get_monitors() {
        if let Ok(mons) = Kde::get_monitors() {
            println!("KDE kscreen-doctor monitors: {:?}", mons);
            assert!(!mons.is_empty());
        }
    }

    #[test]
    fn test_kde_fullscreen_check() {
        let is_fs = Kde::is_fullscreen_active();
        println!("KDE is_fullscreen_active: {}", is_fs);
    }
}
