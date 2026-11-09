use anyhow::{Context, Result};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};


use crate::discovery::{Discovery, Wallpaper};
use crate::hyprland::{HyprEvent, Hyprland, Monitor};
use crate::ipc::{get_socket_path, IpcRequest, IpcResponse};
use crate::renderer::RenderManager;

pub struct DaemonState {
    pub wallpapers: Vec<Wallpaper>,
    pub render_manager: RenderManager,
    pub monitors: Vec<Monitor>,
    pub current_indices: HashMap<String, usize>,
}

impl DaemonState {
    pub fn new() -> Self {
        let wallpapers = Discovery::discover_all(&[]);
        let monitors = Hyprland::get_monitors().unwrap_or_else(|e| {
            warn!("Could not fetch Hyprland monitors: {}", e);
            vec![]
        });

        Self {
            wallpapers,
            render_manager: RenderManager::new(),
            monitors,
            current_indices: HashMap::new(),
        }
    }

    pub fn rescan(&mut self) {
        self.wallpapers = Discovery::discover_all(&[]);
        if let Ok(mons) = Hyprland::get_monitors() {
            self.monitors = mons;
        }
        info!("Rescanned library: {} wallpapers", self.wallpapers.len());
    }

    pub fn find_wallpaper(&self, query: &str) -> Option<&Wallpaper> {
        // Direct ID match
        if let Some(wp) = self.wallpapers.iter().find(|w| w.id == query) {
            return Some(wp);
        }
        // Case-insensitive title match
        if let Some(wp) = self
            .wallpapers
            .iter()
            .find(|w| w.title.eq_ignore_ascii_case(query))
        {
            return Some(wp);
        }
        // Substring match
        self.wallpapers
            .iter()
            .find(|w| w.title.to_lowercase().contains(&query.to_lowercase()))
    }

    pub async fn apply_by_query(&mut self, query: &str, monitor: Option<&str>) -> Result<Wallpaper> {
        let wp = self
            .find_wallpaper(query)
            .cloned()
            .with_context(|| format!("Wallpaper '{}' not found", query))?;

        let mon_target = monitor.unwrap_or("*");
        if mon_target == "*" {
            if self.monitors.is_empty() {
                self.render_manager.apply_wallpaper("DP-1", &wp).await?;
            } else {
                for m in &self.monitors {
                    self.render_manager.apply_wallpaper(&m.name, &wp).await?;
                }
            }
        } else {
            self.render_manager.apply_wallpaper(mon_target, &wp).await?;
        }

        Ok(wp)
    }

    pub async fn cycle_wallpaper(&mut self, monitor: &str, step: isize) -> Result<Option<Wallpaper>> {
        if self.wallpapers.is_empty() {
            return Ok(None);
        }

        let len = self.wallpapers.len();
        let current_idx = self.current_indices.entry(monitor.to_string()).or_insert(0);
        let next_idx = if step >= 0 {
            (*current_idx + step as usize) % len
        } else {
            (*current_idx + len - ((-step) as usize % len)) % len
        };

        *current_idx = next_idx;
        let wp = self.wallpapers[next_idx].clone();
        self.render_manager.apply_wallpaper(monitor, &wp).await?;
        Ok(Some(wp))
    }

    pub async fn random_wallpaper(&mut self, monitor: &str) -> Result<Option<Wallpaper>> {
        if self.wallpapers.is_empty() {
            return Ok(None);
        }

        let idx = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize
            % self.wallpapers.len();

        self.current_indices.insert(monitor.to_string(), idx);
        let wp = self.wallpapers[idx].clone();
        self.render_manager.apply_wallpaper(monitor, &wp).await?;
        Ok(Some(wp))
    }
}

pub struct Daemon;

impl Daemon {
    pub async fn run() -> Result<()> {
        let socket_path = get_socket_path();
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        info!("Starting wengine daemon on {:?}", socket_path);
        let listener = UnixListener::bind(&socket_path)?;

        // Setup Hyprland window rules
        Hyprland::setup_window_rules();

        let state = Arc::new(Mutex::new(DaemonState::new()));

        // Start Hyprland event listener
        let (hypr_tx, mut hypr_rx) = mpsc::unbounded_channel();
        let _hypr_handle = Hyprland::start_event_listener(hypr_tx);

        // Hyprland event handler loop
        let state_hypr = Arc::clone(&state);
        tokio::spawn(async move {
            while let Some(event) = hypr_rx.recv().await {
                match event {
                    HyprEvent::Fullscreen(is_full) => {
                        let mut s = state_hypr.lock().await;
                        if is_full {
                            info!("Fullscreen active: pausing wallpapers");
                            let _ = s.render_manager.pause_all().await;
                        } else {
                            info!("Fullscreen exited: resuming wallpapers");
                            let _ = s.render_manager.resume_all().await;
                        }
                    }
                    HyprEvent::MonitorAdded(name) => {
                        info!("Monitor connected: {}", name);
                        let mut s = state_hypr.lock().await;
                        if let Ok(mons) = Hyprland::get_monitors() {
                            s.monitors = mons;
                        }
                    }
                    HyprEvent::MonitorRemoved(name) => {
                        info!("Monitor disconnected: {}", name);
                        let mut s = state_hypr.lock().await;
                        if let Ok(mons) = Hyprland::get_monitors() {
                            s.monitors = mons;
                        }
                    }
                    _ => {}
                }
            }
        });

        info!("wengine daemon initialized and waiting for connections");

        // IPC Server Loop
        while let Ok((stream, _)) = listener.accept().await {
            let state_clone = Arc::clone(&state);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();

                if let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        return;
                    }

                    let resp = match serde_json::from_str::<IpcRequest>(&line) {
                        Ok(req) => Self::handle_request(state_clone, req).await,
                        Err(e) => IpcResponse::Error {
                            message: format!("Invalid request JSON: {}", e),
                        },
                    };

                    let mut out = serde_json::to_vec(&resp).unwrap_or_default();
                    out.push(b'\n');
                    let mut stream = reader.into_inner();
                    let _ = stream.write_all(&out).await;
                }
            });
        }

        Ok(())
    }

    async fn handle_request(state: Arc<Mutex<DaemonState>>, req: IpcRequest) -> IpcResponse {
        let mut s = state.lock().await;

        match req {
            IpcRequest::List => {
                let list = json!(s.wallpapers);
                IpcResponse::Ok { data: list }
            }
            IpcRequest::Apply { id, monitor } => {
                match s.apply_by_query(&id, monitor.as_deref()).await {
                    Ok(wp) => IpcResponse::Ok {
                        data: json!({
                            "applied": wp.title,
                            "id": wp.id,
                            "type": wp.wallpaper_type,
                            "monitor": monitor.unwrap_or_else(|| "*".to_string())
                        }),
                    },
                    Err(e) => IpcResponse::Error {
                        message: e.to_string(),
                    },
                }
            }
            IpcRequest::Status => {
                let active = s.render_manager.get_active_wallpapers();
                let status_data = json!({
                    "paused": s.render_manager.is_paused(),
                    "volume": s.render_manager.volume(),
                    "muted": s.render_manager.is_muted(),
                    "active_wallpapers": active,
                    "monitors": s.monitors,
                    "total_wallpapers": s.wallpapers.len(),
                });
                IpcResponse::Ok { data: status_data }
            }
            IpcRequest::Pause => {
                let _ = s.render_manager.pause_all().await;
                IpcResponse::Ok {
                    data: json!({ "paused": true }),
                }
            }
            IpcRequest::Resume => {
                let _ = s.render_manager.resume_all().await;
                IpcResponse::Ok {
                    data: json!({ "paused": false }),
                }
            }
            IpcRequest::Toggle => match s.render_manager.toggle_all().await {
                Ok(paused) => IpcResponse::Ok {
                    data: json!({ "paused": paused }),
                },
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            },
            IpcRequest::Volume { value } => {
                let _ = s.render_manager.set_volume(value).await;
                IpcResponse::Ok {
                    data: json!({ "volume": value }),
                }
            }
            IpcRequest::Mute { value } => {
                let _ = s.render_manager.set_mute(value).await;
                IpcResponse::Ok {
                    data: json!({ "muted": value }),
                }
            }
            IpcRequest::Next { monitor } => {
                let mon = monitor.unwrap_or_else(|| "DP-1".to_string());
                match s.cycle_wallpaper(&mon, 1).await {
                    Ok(Some(wp)) => IpcResponse::Ok { data: json!(wp) },
                    Ok(None) => IpcResponse::Error {
                        message: "No wallpapers available".to_string(),
                    },
                    Err(e) => IpcResponse::Error {
                        message: e.to_string(),
                    },
                }
            }
            IpcRequest::Prev { monitor } => {
                let mon = monitor.unwrap_or_else(|| "DP-1".to_string());
                match s.cycle_wallpaper(&mon, -1).await {
                    Ok(Some(wp)) => IpcResponse::Ok { data: json!(wp) },
                    Ok(None) => IpcResponse::Error {
                        message: "No wallpapers available".to_string(),
                    },
                    Err(e) => IpcResponse::Error {
                        message: e.to_string(),
                    },
                }
            }
            IpcRequest::Random { monitor } => {
                let mon = monitor.unwrap_or_else(|| "DP-1".to_string());
                match s.random_wallpaper(&mon).await {
                    Ok(Some(wp)) => IpcResponse::Ok { data: json!(wp) },
                    Ok(None) => IpcResponse::Error {
                        message: "No wallpapers available".to_string(),
                    },
                    Err(e) => IpcResponse::Error {
                        message: e.to_string(),
                    },
                }
            }
            IpcRequest::Rescan => {
                s.rescan();
                IpcResponse::Ok {
                    data: json!({ "total_wallpapers": s.wallpapers.len() }),
                }
            }
        }
    }
}
