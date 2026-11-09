use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {

    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "refreshRate")]
    pub refresh_rate: f64,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyprEvent {
    Fullscreen(bool),
    FocusedMon(String),
    Workspace(String),
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
                "monitoradded" => HyprEvent::MonitorAdded(arg.to_string()),
                "monitorremoved" => HyprEvent::MonitorRemoved(arg.to_string()),
                other => HyprEvent::Other(format!("{}>>{}", other, arg)),
            }
        } else {
            HyprEvent::Other(line.to_string())
        }
    }

    /// Setup Hyprland window rules dynamically to treat background windows as bottom layers
    pub fn setup_window_rules() {
        let rules = [
            "float, class:^(wengine-bg-.*)$",
            "pin, class:^(wengine-bg-.*)$",
            "noborder, class:^(wengine-bg-.*)$",
            "noinitialfocus, class:^(wengine-bg-.*)$",
            "nofocus, class:^(wengine-bg-.*)$",
            "noshadow, class:^(wengine-bg-.*)$",
            "nodim, class:^(wengine-bg-.*)$",
            "layer:background, class:^(wengine-bg-.*)$",
        ];

        for rule in rules {
            let _ = Command::new("hyprctl")
                .args(["keyword", "windowrulev2", rule])
                .output();
        }
    }
}
