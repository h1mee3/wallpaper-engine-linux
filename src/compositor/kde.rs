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
