use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::compositor::{Compositor, CompositorEvent, Monitor};
use crate::discovery::{Discovery, Wallpaper};
use crate::ipc::{get_socket_path, IpcRequest, IpcResponse, PauseMode};
use crate::renderer::RenderManager;
use crate::watcher::SteamWatcher;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedMonitorState {
    pub id: String,
    pub title: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedState {
    pub active_wallpapers: HashMap<String, PersistedMonitorState>,
    pub scaling_mode: Option<crate::renderer::ScalingMode>,
    pub volume: Option<u32>,
    pub muted: Option<bool>,
}

pub struct DaemonState {
    pub wallpapers: Vec<Wallpaper>,
    pub render_manager: RenderManager,
    pub monitors: Vec<Monitor>,
    pub current_indices: HashMap<String, usize>,
    pub pause_mode: PauseMode,
    pub auto_paused: bool,
}

impl DaemonState {
    pub fn new(pause_mode: PauseMode) -> Self {
        let wallpapers = Discovery::discover_all(&[]);
        let monitors = Compositor::get_monitors().unwrap_or_else(|e| {
            warn!("Could not fetch displays: {}", e);
            vec![]
        });

        Self {
            wallpapers,
            render_manager: RenderManager::new(),
            monitors,
            current_indices: HashMap::new(),
            pause_mode,
            auto_paused: false,
        }
    }

    pub fn rescan(&mut self) {
        self.wallpapers = Discovery::discover_all(&[]);
        if let Ok(mons) = Compositor::get_monitors() {
            self.monitors = mons;
        }
        info!("Rescanned library: {} wallpapers, {} displays", self.wallpapers.len(), self.monitors.len());
    }

    /// Save active wallpapers and settings to persistent state (~/.config/wengine/state.json)
    pub fn save_state(&self) {
        let config_dir = match dirs::config_dir() {
            Some(d) => d.join("wengine"),
            None => return,
        };
        let _ = std::fs::create_dir_all(&config_dir);
        let state_path = config_dir.join("state.json");

        let mut active = HashMap::new();
        for (mon, wp) in self.render_manager.get_active_wallpapers() {
            active.insert(
                mon.clone(),
                PersistedMonitorState {
                    id: wp.id.clone(),
                    title: wp.title.clone(),
                    path: if wp.is_web() {
                        wp.resolve_web_path()
                    } else {
                        wp.resolve_playable_path()
                    },
                },
            );
        }

        let state = PersistedState {
            active_wallpapers: active,
            scaling_mode: Some(self.render_manager.scaling_mode()),
            volume: Some(self.render_manager.volume()),
            muted: Some(self.render_manager.is_muted()),
        };

        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&state_path, json);
            debug!("Saved wallpaper state to {:?}", state_path);
        }
    }

    /// Restore persistent wallpaper state and settings on daemon startup/restart
    pub async fn restore_state(&mut self) {
        let state_path = match dirs::config_dir() {
            Some(d) => d.join("wengine").join("state.json"),
            None => return,
        };

        if !state_path.exists() {
            debug!("No saved state file found at {:?}", state_path);
            return;
        }

        let content = match std::fs::read_to_string(&state_path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Could not read state.json: {}", e);
                return;
            }
        };

        let state: PersistedState = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to parse state.json: {}", e);
                return;
            }
        };

        if let Some(scaling) = state.scaling_mode {
            let _ = self.render_manager.set_scaling_mode(scaling).await;
        }
        if let Some(volume) = state.volume {
            let _ = self.render_manager.set_volume(volume).await;
        }
        if let Some(muted) = state.muted {
            let _ = self.render_manager.set_mute(muted).await;
        }

        if state.active_wallpapers.is_empty() {
            return;
        }

        info!(
            "Restoring {} active wallpaper(s) from persistent state",
            state.active_wallpapers.len()
        );

        // If Wayland outputs are still initializing during login, poll up to 3 seconds
        for _ in 0..10 {
            if !self.monitors.is_empty() {
                break;
            }
            if let Ok(mons) = Compositor::get_monitors() {
                if !mons.is_empty() {
                    self.monitors = mons;
                    break;
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        }

        for (mon, saved_wp) in state.active_wallpapers {
            let mut applied = self.apply_by_query(&saved_wp.id, Some(&mon)).await;
            if applied.is_err() {
                applied = self.apply_by_query(&saved_wp.title, Some(&mon)).await;
            }
            match applied {
                Ok(wp) => {
                    info!("Restored wallpaper '{}' on monitor '{}'", wp.title, mon);
                }
                Err(e) => {
                    warn!(
                        "Could not restore wallpaper '{}' ({}) on monitor '{}': {}",
                        saved_wp.title, saved_wp.id, mon, e
                    );
                }
            }
        }
    }

    /// Retrieve the exact resolution and coordinate offset for a monitor
    pub fn get_monitor_geometry(&self, mon_name: &str) -> (u32, u32, i32, i32) {
        if let Some(m) = self.monitors.iter().find(|m| m.name == mon_name) {
            (m.logical_width(), m.logical_height(), m.x, m.y)
        } else if let Some(first) = self.monitors.first() {
            (first.logical_width(), first.logical_height(), first.x, first.y)
        } else {
            (1920, 1080, 0, 0)
        }
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
                let (w, h, x, y) = self.get_monitor_geometry("*");
                self.render_manager
                    .apply_wallpaper("*", &wp, w, h, x, y)
                    .await?;
            } else {
                for m in &self.monitors {
                    let (w, h, x, y) = (m.logical_width(), m.logical_height(), m.x, m.y);
                    self.render_manager
                        .apply_wallpaper(&m.name, &wp, w, h, x, y)
                        .await?;
                }
            }
        } else {
            let (w, h, x, y) = self.get_monitor_geometry(mon_target);
            self.render_manager
                .apply_wallpaper(mon_target, &wp, w, h, x, y)
                .await?;
        }

        self.save_state();
        // Check if new wallpaper should be auto-paused according to current pause mode
        let _ = self.evaluate_auto_pause().await;
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
        let (w, h, x, y) = self.get_monitor_geometry(monitor);
        self.render_manager
            .apply_wallpaper(monitor, &wp, w, h, x, y)
            .await?;
        self.save_state();
        let _ = self.evaluate_auto_pause().await;
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
        let (w, h, x, y) = self.get_monitor_geometry(monitor);
        self.render_manager
            .apply_wallpaper(monitor, &wp, w, h, x, y)
            .await?;
        self.save_state();
        let _ = self.evaluate_auto_pause().await;
        Ok(Some(wp))
    }

    /// Evaluate auto-pause and power-save conditions based on window layers and active workspace
    pub async fn evaluate_auto_pause(&mut self) -> Result<()> {
        let (has_fullscreen, has_apps_above) = Compositor::check_apps_above();

        debug!(
            "Evaluating auto-pause (mode: {:?}, fullscreen: {}, apps_above: {})",
            self.pause_mode, has_fullscreen, has_apps_above
        );

        match self.pause_mode {
            PauseMode::Always => {
                // User wants wallpaper running in background continuously, never auto-pause
                if self.auto_paused {
                    self.auto_paused = false;
                    self.render_manager.resume_all().await?;
                }
            }
            PauseMode::Fullscreen => {
                if has_fullscreen {
                    if !self.auto_paused {
                        info!("Fullscreen app active: auto-pausing wallpaper");
                        self.auto_paused = true;
                        self.render_manager.pause_all().await?;
                    }
                } else if self.auto_paused {
                    info!("Fullscreen app exited: auto-resuming wallpaper");
                    self.auto_paused = false;
                    self.render_manager.resume_all().await?;
                }
            }
            PauseMode::PowerSave => {
                let should_pause = has_fullscreen || has_apps_above;
                if should_pause {
                    if !self.auto_paused {
                        info!("Power-save active (app above wallpaper): pausing wallpaper to save 100% resources");
                        self.auto_paused = true;
                        self.render_manager.pause_all().await?;
                    }
                } else if self.auto_paused {
                    info!("Desktop visible (no apps above wallpaper): resuming wallpaper");
                    self.auto_paused = false;
                    self.render_manager.resume_all().await?;
                }
            }
        }
        Ok(())
    }
}

pub struct Daemon;

impl Daemon {
    pub async fn run(initial_pause_mode: PauseMode) -> Result<()> {
        let socket_path = get_socket_path();
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        info!("Starting wengine daemon on {:?} (pause_mode: {:?})", socket_path, initial_pause_mode);
        let listener = UnixListener::bind(&socket_path)?;

        // Ensure Wayland display & session environment are ready (handles SDDM/GDM/systemd login)
        Compositor::wait_for_wayland_env().await;

        // Setup compositor window/layer rules
        Compositor::setup_compositor_rules();

        let state = Arc::new(Mutex::new(DaemonState::new(initial_pause_mode)));

        // Restore saved wallpaper state immediately on startup/restart
        {
            let mut s = state.lock().await;
            s.restore_state().await;
        }

        // Start Steam Workshop and config.json watcher
        let state_watcher = Arc::clone(&state);
        let _watcher_handle = SteamWatcher::start(state_watcher);

        // Start unified compositor event listener (auto-pause on fullscreen, monitor hotplug, workspace change)
        let (comp_tx, mut comp_rx) = mpsc::unbounded_channel();
        let _comp_handle = Compositor::start_event_listener(comp_tx);

        // Compositor event handler loop
        let state_comp = Arc::clone(&state);
        tokio::spawn(async move {
            while let Some(event) = comp_rx.recv().await {
                match event {
                    CompositorEvent::Fullscreen(_)
                    | CompositorEvent::WorkspaceChanged(_)
                    | CompositorEvent::WindowChanged => {
                        let mut s = state_comp.lock().await;
                        let _ = s.evaluate_auto_pause().await;
                    }
                    CompositorEvent::MonitorAdded(name) => {
                        info!("Monitor connected: {}", name);
                        let mut s = state_comp.lock().await;
                        if let Ok(mons) = Compositor::get_monitors() {
                            s.monitors = mons;
                        }
                        Compositor::setup_compositor_rules();
                    }
                    CompositorEvent::MonitorRemoved(name) => {
                        info!("Monitor disconnected: {}", name);
                        let mut s = state_comp.lock().await;
                        if let Ok(mons) = Compositor::get_monitors() {
                            s.monitors = mons;
                        }
                    }
                    _ => {}
                }
            }
        });

        info!("wengine daemon initialized and waiting for connections");

        // Initial auto-pause evaluation
        {
            let mut s = state.lock().await;
            let _ = s.evaluate_auto_pause().await;
        }

        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // IPC Server Loop
        loop {
            tokio::select! {
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _)) => {
                            let state_clone = Arc::clone(&state);
                            let shutdown_tx_clone = shutdown_tx.clone();
                            tokio::spawn(async move {
                                let mut reader = BufReader::new(stream);
                                let mut line = String::new();

                                if let Ok(n) = reader.read_line(&mut line).await {
                                    if n == 0 {
                                        return;
                                    }

                                    let (resp, should_stop) = match serde_json::from_str::<IpcRequest>(&line) {
                                        Ok(IpcRequest::Stop) => {
                                            (IpcResponse::Ok {
                                                data: json!({"message": "Daemon shutting down"}),
                                            }, true)
                                        }
                                        Ok(req) => (Self::handle_request(state_clone, req).await, false),
                                        Err(e) => (IpcResponse::Error {
                                            message: format!("Invalid request JSON: {}", e),
                                        }, false),
                                    };

                                    let mut out = serde_json::to_vec(&resp).unwrap_or_default();
                                    out.push(b'\n');
                                    let mut stream = reader.into_inner();
                                    let _ = stream.write_all(&out).await;

                                    if should_stop {
                                        let _ = shutdown_tx_clone.send(()).await;
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            warn!("Error accepting IPC connection: {}", e);
                        }
                    }
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM (systemd / stop request), initiating graceful shutdown");
                    break;
                }
                _ = sigint.recv() => {
                    info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
                    break;
                }
                _ = shutdown_rx.recv() => {
                    info!("Received IPC Stop command, initiating graceful shutdown");
                    break;
                }
            }
        }

        info!("Shutting down wengine daemon...");
        _watcher_handle.abort();
        if let Some(h) = _comp_handle {
            h.abort();
        }

        // Cleanly stop all active wallpaper renderers
        {
            let mut s = state.lock().await;
            s.render_manager.stop_all();
        }

        // Clean up socket file
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        info!("wengine daemon shutdown complete");
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
                let (has_fullscreen, has_apps) = Compositor::check_apps_above();
                let status_data = json!({
                    "paused": s.render_manager.is_paused(),
                    "auto_paused": s.auto_paused,
                    "pause_mode": s.pause_mode,
                    "power_save": s.pause_mode == PauseMode::PowerSave,
                    "apps_above": has_apps,
                    "has_fullscreen": has_fullscreen,
                    "volume": s.render_manager.volume(),
                    "muted": s.render_manager.is_muted(),
                    "scaling_mode": s.render_manager.scaling_mode(),
                    "desktop_environment": Compositor::current_de().name(),
                    "supports_layer_shell": Compositor::current_de().supports_layer_shell(),
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
            IpcRequest::SetPauseMode { mode } => {
                s.pause_mode = mode;
                let _ = s.evaluate_auto_pause().await;
                IpcResponse::Ok {
                    data: json!({
                        "pause_mode": s.pause_mode,
                        "power_save": s.pause_mode == PauseMode::PowerSave
                    }),
                }
            }
            IpcRequest::PowerSave { enabled } => {
                s.pause_mode = if enabled {
                    PauseMode::PowerSave
                } else {
                    PauseMode::Fullscreen
                };
                let _ = s.evaluate_auto_pause().await;
                IpcResponse::Ok {
                    data: json!({
                        "power_save": enabled,
                        "pause_mode": s.pause_mode
                    }),
                }
            }
            IpcRequest::Volume { value } => {
                let _ = s.render_manager.set_volume(value).await;
                s.save_state();
                IpcResponse::Ok {
                    data: json!({ "volume": value }),
                }
            }
            IpcRequest::Mute { value } => {
                let _ = s.render_manager.set_mute(value).await;
                s.save_state();
                IpcResponse::Ok {
                    data: json!({ "muted": value }),
                }
            }
            IpcRequest::Scaling { mode } => {
                let _ = s.render_manager.set_scaling_mode(mode).await;
                s.save_state();
                IpcResponse::Ok {
                    data: json!({ "scaling_mode": mode }),
                }
            }
            IpcRequest::Next { monitor } => {
                let mon = monitor.unwrap_or_else(|| {
                    s.monitors.first().map(|m| m.name.clone()).unwrap_or_else(|| "*".to_string())
                });
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
                let mon = monitor.unwrap_or_else(|| {
                    s.monitors.first().map(|m| m.name.clone()).unwrap_or_else(|| "*".to_string())
                });
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
                let mon = monitor.unwrap_or_else(|| {
                    s.monitors.first().map(|m| m.name.clone()).unwrap_or_else(|| "*".to_string())
                });
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
            IpcRequest::Stop => IpcResponse::Ok {
                data: json!({ "message": "Stopping daemon" }),
            },
        }
    }
}
