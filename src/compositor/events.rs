use tokio::sync::mpsc;
use tracing::info;

use super::detection::{detect_desktop_environment, DesktopEnvironment};
use super::sway::Sway;
use crate::hyprland::{HyprEvent, Hyprland};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositorEvent {
    Fullscreen(bool),
    MonitorAdded(String),
    MonitorRemoved(String),
    WorkspaceChanged(String),
    WindowChanged,
    Other(String),
}

pub struct CompositorEvents;

impl CompositorEvents {
    /// Start listening to compositor events (auto-pause on fullscreen, monitor hotplug)
    /// automatically adapting to whatever DE / WM is currently running.
    pub fn start_listener(tx: mpsc::UnboundedSender<CompositorEvent>) -> Option<tokio::task::JoinHandle<()>> {
        let de = detect_desktop_environment();
        info!("Setting up event listener for desktop environment: {}", de.name());

        match de {
            DesktopEnvironment::Hyprland => {
                let (hypr_tx, mut hypr_rx) = mpsc::unbounded_channel();
                let _handle = Hyprland::start_event_listener(hypr_tx);
                let tx_clone = tx.clone();

                Some(tokio::spawn(async move {
                    while let Some(evt) = hypr_rx.recv().await {
                        let mapped = match evt {
                            HyprEvent::Fullscreen(b) => CompositorEvent::Fullscreen(b),
                            HyprEvent::MonitorAdded(m) => CompositorEvent::MonitorAdded(m),
                            HyprEvent::MonitorRemoved(m) => CompositorEvent::MonitorRemoved(m),
                            HyprEvent::Workspace(w) => CompositorEvent::WorkspaceChanged(w),
                            HyprEvent::WindowChanged => CompositorEvent::WindowChanged,
                            HyprEvent::Other(o) => CompositorEvent::Other(o),
                            _ => continue,
                        };
                        if tx_clone.send(mapped).is_err() {
                            break;
                        }
                    }
                }))
            }
            DesktopEnvironment::Sway => Sway::start_event_listener(tx),
            _ => {
                // Compositor has no specialized event socket or is generic Wayland.
                // Runs smoothly without needing an event listener.
                info!("Desktop environment '{}' uses native layer surface management", de.name());
                None
            }
        }
    }
}
