use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

pub use crate::compositor::Monitor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyprEvent {
    Fullscreen(bool),
    FocusedMon(String),
    Workspace(String),
    WindowChanged,
    MonitorAdded(String),
    MonitorRemoved(String),
    Other(String),
}

pub struct Hyprland;

impl Hyprland {
    /// Query all active monitors from Hyprland via `hyprctl monitors -j`
    pub fn get_monitors() -> Result<Vec<Monitor>> {
        let output = Command::new("hyprctl")
            .args(["monitors", "-j"])
            .output()
            .context("Failed to run hyprctl monitors -j")?;

        if !output.status.success() {
            anyhow::bail!(
                "hyprctl exited with error: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let monitors: Vec<Monitor> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse hyprctl monitors JSON")?;
        Ok(monitors)
    }

    /// Resolve Hyprland's socket2 path: $XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock
    pub fn socket2_path() -> Option<PathBuf> {
        let xdg_runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
        let his = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let path = PathBuf::from(xdg_runtime).join("hypr").join(his).join(".socket2.sock");
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    /// Start listening to Hyprland socket2 events asynchronously.
    /// Sends parsed events to the provided mpsc sender.
    pub fn start_event_listener(tx: mpsc::UnboundedSender<HyprEvent>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let socket_path = match Self::socket2_path() {
                Some(p) => p,
                None => {
                    warn!("Hyprland socket2 not found, event listener disabled");
                    return;
                }
            };

            info!("Connecting to Hyprland event socket: {:?}", socket_path);
            let stream = match UnixStream::connect(&socket_path).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to connect to Hyprland socket2: {}", e);
                    return;
                }
            };

            let mut reader = BufReader::new(stream).lines();
            info!("Hyprland event listener active");

            while let Ok(Some(line)) = reader.next_line().await {
                debug!("Hyprland event raw: {}", line);
                let event = Self::parse_event(&line);
                if tx.send(event).is_err() {
                    break;
                }
            }
        })
    }

    /// Parse a single Hyprland socket2 event line
    pub fn parse_event(line: &str) -> HyprEvent {
        if let Some((evt, arg)) = line.split_once(">>") {
            match evt {
                "fullscreen" => {
                    let is_full = arg == "1" || arg.starts_with('1');
                    HyprEvent::Fullscreen(is_full)
                }
                "focusedmon" => {
                    let mon = arg.split(',').next().unwrap_or(arg).to_string();
                    HyprEvent::FocusedMon(mon)
                }
                "workspace" => HyprEvent::Workspace(arg.to_string()),
                "openwindow" | "closewindow" | "movewindow" | "activewindow" => {
                    HyprEvent::WindowChanged
                }
                "monitoradded" => HyprEvent::MonitorAdded(arg.to_string()),
                "monitorremoved" => HyprEvent::MonitorRemoved(arg.to_string()),
                other => HyprEvent::Other(format!("{}>>{}", other, arg)),
            }
        } else {
            HyprEvent::Other(line.to_string())
        }
    }

    /// Check if there are any windows or fullscreen apps on the active workspace or covering layers
    pub fn check_apps_above() -> (bool, bool) {
        let mut has_fullscreen = false;
        let mut has_apps = false;

        // 1. Check active workspace for windows & fullscreen flag
        if let Ok(output) = Command::new("hyprctl").args(["activeworkspace", "-j"]).output() {
            if output.status.success() {
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                    if let Some(w) = val.get("windows").and_then(|v| v.as_u64()) {
                        if w > 0 {
                            has_apps = true;
                        }
                    }
                    if let Some(f) = val.get("hasfullscreen").and_then(|v| v.as_bool()) {
                        if f {
                            has_fullscreen = true;
                        }
                    }
                }
            }
        }

        // 2. Check surface layers for overlay (layer 3) covering the screen
        if let Ok(output) = Command::new("hyprctl").args(["layers", "-j"]).output() {
            if output.status.success() {
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                    if let Some(obj) = val.as_object() {
                        for (_mon, data) in obj {
                            if let Some(levels) = data.get("levels").and_then(|l| l.as_object()) {
                                if let Some(overlay) = levels.get("3").and_then(|v| v.as_array()) {
                                    for surface in overlay {
                                        let w = surface.get("w").and_then(|v| v.as_u64()).unwrap_or(0);
                                        let h = surface.get("h").and_then(|v| v.as_u64()).unwrap_or(0);
                                        if w >= 1280 && h >= 720 {
                                            has_fullscreen = true;
                                            has_apps = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (has_fullscreen, has_apps)
    }

    /// Setup any necessary layer rules for wengine-wallpaper
    pub fn setup_window_rules() {
        // With native wlr-layer-shell, wengine-wallpaper automatically runs on Layer level 0 (background).
        // No toplevel window rules needed.
    }

    /// Send a window to the bottom of the z-order
    pub fn send_to_bottom(class_name: &str) {
        let code = format!(
            "hl.dsp.window.alter_zorder({{ mode = 'bottom', window = 'class:{}' }})",
            class_name
        );
        let _ = Command::new("hyprctl").args(["repl", &code]).output();
    }
}
