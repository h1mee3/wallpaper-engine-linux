use anyhow::{Context, Result};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use wayland_client::{
    event_created_child,
    protocol::{wl_output, wl_registry},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::events::CompositorEvent;
use super::monitor::Monitor;

#[derive(Default)]
struct OutputData {
    pub _id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh_rate: f64,
    pub scale: f64,
    pub description: String,
}

struct WaylandState {
    outputs: Vec<(u32, wl_output::WlOutput, OutputData)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_output" {
                let bind_ver = version.min(4);
                let output = registry.bind::<wl_output::WlOutput, _, _>(name, bind_ver, qh, ());
                state.outputs.push((
                    name,
                    output,
                    OutputData {
                        _id: name,
                        scale: 1.0,
                        ..Default::default()
                    },
                ));
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for WaylandState {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some((_, _, data)) = state.outputs.iter_mut().find(|(_, o, _)| o == output) {
            match event {
                wl_output::Event::Geometry { x, y, .. } => {
                    data.x = x;
                    data.y = y;
                }
                wl_output::Event::Mode {
                    flags,
                    width,
                    height,
                    refresh,
                } => {
                    let is_current = match flags {
                        wayland_client::WEnum::Value(f) => f.contains(wl_output::Mode::Current),
                        _ => false,
                    };
                    if is_current || data.width == 0 {
                        data.width = width as u32;
                        data.height = height as u32;
                        data.refresh_rate = refresh as f64 / 1000.0;
                    }
                }
                wl_output::Event::Scale { factor } => {
                    if factor > 0 {
                        data.scale = factor as f64;
                    }
                }
                wl_output::Event::Name { name } => {
                    data.name = name;
                }
                wl_output::Event::Description { description } => {
                    data.description = description;
                }
                _ => {}
            }
        }
    }
}

pub fn query_wayland_monitors() -> Result<Vec<Monitor>> {
    let conn = Connection::connect_to_env().context("Failed to connect to Wayland display")?;
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = WaylandState { outputs: Vec::new() };

    let _registry = conn.display().get_registry(&qh, ());
    // Roundtrip to bind globals
    event_queue.roundtrip(&mut state).context("Wayland roundtrip failed")?;
    // Second roundtrip to receive output events (name, mode, geometry)
    event_queue.roundtrip(&mut state).context("Wayland output events roundtrip failed")?;

    let mut monitors = Vec::new();
    for (idx, (_, _, data)) in state.outputs.into_iter().enumerate() {
        let name = if !data.name.is_empty() {
            data.name
        } else if !data.description.is_empty() {
            data.description
        } else {
            format!("output-{}", idx)
        };

        monitors.push(Monitor {
            id: idx as u32,
            name,
            width: if data.width > 0 { data.width } else { 1920 },
            height: if data.height > 0 { data.height } else { 1080 },
            x: data.x,
            y: data.y,
            scale: if data.scale > 0.0 { data.scale } else { 1.0 },
            refresh_rate: if data.refresh_rate > 0.0 { data.refresh_rate } else { 60.0 },
            focused: idx == 0,
        });
    }

    Ok(monitors)
}

#[derive(Default, Clone)]
struct ToplevelInfo {
    app_id: String,
    maximized: bool,
    minimized: bool,
    activated: bool,
    fullscreen: bool,
}

struct ToplevelQueryState {
    manager: Option<ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<u32, ToplevelInfo>,
    output_names: HashMap<u32, String>,
    event_tx: Option<mpsc::UnboundedSender<CompositorEvent>>,
    last_fullscreen: bool,
}

impl ToplevelQueryState {
    fn evaluate(&self) -> (bool, bool) {
        let mut has_fullscreen = false;
        let mut has_apps_above = false;

        for info in self.toplevels.values() {
            if info.app_id == "wengine-wallpaper" {
                continue;
            }
            if info.minimized {
                continue;
            }
            if info.fullscreen && info.activated {
                has_fullscreen = true;
            } else if info.fullscreen && self.toplevels.len() == 1 {
                has_fullscreen = true;
            }
            if info.activated || info.maximized {
                has_apps_above = true;
            }
        }

        (has_fullscreen, has_apps_above)
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for ToplevelQueryState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface == "zwlr_foreign_toplevel_manager_v1" && state.manager.is_none() {
                    let mgr = registry.bind::<ZwlrForeignToplevelManagerV1, _, _>(
                        name,
                        version.min(3),
                        qh,
                        (),
                    );
                    state.manager = Some(mgr);
                } else if interface == "wl_output" && state.event_tx.is_some() {
                    state.output_names.entry(name).or_insert_with(|| format!("output-{}", name));
                    if let Some(ref tx) = state.event_tx {
                        let _ = tx.send(CompositorEvent::MonitorAdded(format!("output-{}", name)));
                    }
                }
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(out_name) = state.output_names.remove(&name) {
                    if let Some(ref tx) = state.event_tx {
                        let _ = tx.send(CompositorEvent::MonitorRemoved(out_name));
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for ToplevelQueryState {
    event_created_child!(ToplevelQueryState, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ())
    ]);

    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            let id = toplevel.id().protocol_id();
            state.toplevels.insert(id, ToplevelInfo::default());
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for ToplevelQueryState {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = handle.id().protocol_id();
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                if let Some(info) = state.toplevels.get_mut(&id) {
                    info.app_id = app_id;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state: raw_bytes } => {
                if let Some(info) = state.toplevels.get_mut(&id) {
                    info.maximized = false;
                    info.minimized = false;
                    info.activated = false;
                    info.fullscreen = false;
                    for chunk in raw_bytes.chunks_exact(4) {
                        let val = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        match val {
                            0 => info.maximized = true,
                            1 => info.minimized = true,
                            2 => info.activated = true,
                            3 => info.fullscreen = true,
                            _ => {}
                        }
                    }
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {
                if state.event_tx.is_some() {
                    let (is_fs, _) = state.evaluate();
                    if let Some(ref tx) = state.event_tx {
                        if is_fs != state.last_fullscreen {
                            state.last_fullscreen = is_fs;
                            let _ = tx.send(CompositorEvent::Fullscreen(is_fs));
                        }
                        let _ = tx.send(CompositorEvent::WindowChanged);
                    }
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                state.toplevels.remove(&id);
                handle.destroy();
                if state.event_tx.is_some() {
                    let (is_fs, _) = state.evaluate();
                    if let Some(ref tx) = state.event_tx {
                        if is_fs != state.last_fullscreen {
                            state.last_fullscreen = is_fs;
                            let _ = tx.send(CompositorEvent::Fullscreen(is_fs));
                        }
                        let _ = tx.send(CompositorEvent::WindowChanged);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Query open windows via `zwlr_foreign_toplevel_manager_v1` (universal across wlroots/Wayland compositors).
/// Returns `(has_fullscreen, has_apps_above)`.
pub fn query_foreign_toplevels() -> Result<(bool, bool)> {
    let conn = Connection::connect_to_env().context("Failed to connect to Wayland display")?;
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = ToplevelQueryState {
        manager: None,
        toplevels: HashMap::new(),
        output_names: HashMap::new(),
        event_tx: None,
        last_fullscreen: false,
    };

    let _registry = conn.display().get_registry(&qh, ());
    event_queue.roundtrip(&mut state).context("Wayland roundtrip failed")?;

    if state.manager.is_none() {
        anyhow::bail!("Compositor does not expose zwlr_foreign_toplevel_manager_v1");
    }

    // Second roundtrip to receive all initial toplevels and their states
    event_queue.roundtrip(&mut state).context("Wayland toplevel roundtrip failed")?;

    Ok(state.evaluate())
}

/// Universal Wayland event listener using `wl_registry` (monitor hotplug) and
/// `zwlr_foreign_toplevel_manager_v1` (fullscreen & active window tracking).
pub fn start_wayland_event_listener(
    tx: mpsc::UnboundedSender<CompositorEvent>,
) -> Option<tokio::task::JoinHandle<()>> {
    Some(tokio::task::spawn_blocking(move || {
        let conn = match Connection::connect_to_env() {
            Ok(c) => c,
            Err(e) => {
                warn!("Universal Wayland event listener could not connect: {}", e);
                return;
            }
        };

        let mut event_queue = conn.new_event_queue();
        let qh = event_queue.handle();

        let mut state = ToplevelQueryState {
            manager: None,
            toplevels: HashMap::new(),
            output_names: HashMap::new(),
            event_tx: Some(tx.clone()),
            last_fullscreen: false,
        };

        let _registry = conn.display().get_registry(&qh, ());
        if let Err(e) = event_queue.roundtrip(&mut state) {
            warn!("Wayland event listener initial roundtrip failed: {}", e);
            return;
        }
        // Drain initial globals without treating startup outputs as hotplugs
        let _ = event_queue.roundtrip(&mut state);

        info!(
            "Universal Wayland event listener active (foreign_toplevel: {})",
            state.manager.is_some()
        );

        loop {
            if let Err(e) = event_queue.blocking_dispatch(&mut state) {
                debug!("Wayland event listener stopped: {}", e);
                break;
            }
            if tx.is_closed() {
                break;
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_wayland_monitors() {
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            let res = query_wayland_monitors();
            println!("Wayland monitors: {:?}", res);
            assert!(res.is_ok());
            let mons = res.unwrap();
            println!("Found {} monitors:", mons.len());
            for m in &mons {
                println!("  - {} ({}x{} at +{}+{}, scale={})", m.name, m.width, m.height, m.x, m.y, m.scale);
            }
            assert!(!mons.is_empty());
        }
    }
}

