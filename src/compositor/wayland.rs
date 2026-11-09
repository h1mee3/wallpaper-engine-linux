use anyhow::{Context, Result};
use wayland_client::{
    protocol::{wl_output, wl_registry},
    Connection, Dispatch, QueueHandle,
};

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

