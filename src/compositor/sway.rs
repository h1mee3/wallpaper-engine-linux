use anyhow::{Context, Result};
use serde_json::Value;
use std::env;
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::events::CompositorEvent;
use super::monitor::Monitor;

pub struct Sway;

impl Sway {
    pub fn socket_path() -> Option<String> {
        env::var("SWAYSOCK").ok()
    }

    /// Query outputs via `swaymsg -t get_outputs`
    pub fn get_monitors() -> Result<Vec<Monitor>> {
        let output = Command::new("swaymsg")
            .args(["-t", "get_outputs", "--raw"])
            .output()
            .context("Failed to run swaymsg -t get_outputs")?;

        if !output.status.success() {
            anyhow::bail!(
                "swaymsg error: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let json: Value = serde_json::from_slice(&output.stdout)?;
        let array = json
            .as_array()
            .context("Expected JSON array from swaymsg get_outputs")?;

        let mut monitors = Vec::new();
        for (idx, item) in array.iter().enumerate() {
            let name = item["name"].as_str().unwrap_or("?").to_string();
            let rect = &item["rect"];
            let x = rect["x"].as_i64().unwrap_or(0) as i32;
            let y = rect["y"].as_i64().unwrap_or(0) as i32;
            let width = rect["width"].as_u64().unwrap_or(1920) as u32;
            let height = rect["height"].as_u64().unwrap_or(1080) as u32;
            let scale = item["scale"].as_f64().unwrap_or(1.0);
            let focused = item["focused"].as_bool().unwrap_or(false);

            let refresh_rate = item["current_mode"]["refresh"]
                .as_f64()
                .map(|r| r / 1000.0)
                .unwrap_or(60.0);

            monitors.push(Monitor {
                id: idx as u32,
                name,
                width,
                height,
                x,
                y,
                scale,
                refresh_rate,
                focused,
            });
        }

        Ok(monitors)
    }

    /// Start listening to Sway IPC events (fullscreen, outputs)
    pub fn start_event_listener(tx: mpsc::UnboundedSender<CompositorEvent>) -> Option<tokio::task::JoinHandle<()>> {
        let socket_path = Self::socket_path()?;

        Some(tokio::spawn(async move {
            info!("Connecting to Sway IPC socket: {}", socket_path);
            let mut stream = match UnixStream::connect(&socket_path).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to connect to SWAYSOCK: {}", e);
                    return;
                }
            };

            // i3/Sway IPC SUBSCRIBE message:
            // Magic: "i3-ipc" (6 bytes), Length (u32 LE), Type 2 (u32 LE: SUBSCRIBE)
            let payload = b"[\"window\", \"output\"]";
            let mut msg = Vec::new();
            msg.extend_from_slice(b"i3-ipc");
            msg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            msg.extend_from_slice(&2u32.to_le_bytes()); // Type 2 = SUBSCRIBE
            msg.extend_from_slice(payload);

            if let Err(e) = stream.write_all(&msg).await {
                warn!("Failed to subscribe to Sway IPC: {}", e);
                return;
            }

            info!("Sway IPC event listener active");

            loop {
                // Read header: 6 bytes magic + 4 bytes length + 4 bytes type = 14 bytes
                let mut header = [0u8; 14];
                if stream.read_exact(&mut header).await.is_err() {
                    break;
                }

                if &header[0..6] != b"i3-ipc" {
                    warn!("Invalid Sway IPC magic header");
                    break;
                }

                let length = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
                let msg_type = u32::from_le_bytes(header[10..14].try_into().unwrap());

                let mut payload_buf = vec![0u8; length];
                if stream.read_exact(&mut payload_buf).await.is_err() {
                    break;
                }

                // Event types in i3-ipc have bit 31 set: 0x80000000 | event_type
                // Window event = 0x80000003, Output event = 0x80000001
                if msg_type & 0x80000000 != 0 {
                    if let Ok(v) = serde_json::from_slice::<Value>(&payload_buf) {
                        debug!("Sway event: {}", v);
                        let change = v["change"].as_str().unwrap_or_default();

                        if change == "fullscreen_mode" || change == "fullscreen" {
                            let mode = v["container"]["fullscreen_mode"].as_u64().unwrap_or(0);
                            let is_full = mode > 0;
                            let _ = tx.send(CompositorEvent::Fullscreen(is_full));
                        } else if v.get("output").is_some() || change == "unfocused" {
                            // Output hotplug event
                            let name = v["output"]["name"].as_str().unwrap_or("?").to_string();
                            let _ = tx.send(CompositorEvent::MonitorAdded(name));
                        }
                    }
                }
            }
        }))
    }
}
