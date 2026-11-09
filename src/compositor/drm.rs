use std::fs;
use std::path::Path;

use super::monitor::Monitor;

/// Query connected monitors from Linux kernel DRM subsystem (/sys/class/drm)
pub fn query_drm_monitors() -> Vec<Monitor> {
    let mut monitors = Vec::new();
    let drm_path = Path::new("/sys/class/drm");
    if !drm_path.exists() {
        return monitors;
    }

    let entries = match fs::read_dir(drm_path) {
        Ok(e) => e,
        Err(_) => return monitors,
    };

    let mut id = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();

        // Looking for entries like card1-DP-1, card0-HDMI-A-1, etc.
        if !file_name.contains('-') || file_name.starts_with("render") {
            continue;
        }

        let status_path = path.join("status");
        if let Ok(status) = fs::read_to_string(&status_path) {
            if status.trim() == "connected" {
                // Extract clean output name, e.g. card1-DP-1 -> DP-1
                let name = file_name
                    .split_once('-')
                    .map(|(_, port)| port.to_string())
                    .unwrap_or_else(|| file_name.to_string());

                let mut width = 1920;
                let mut height = 1080;

                // Read native mode from /sys/class/drm/.../modes
                let modes_path = path.join("modes");
                if let Ok(modes) = fs::read_to_string(&modes_path) {
                    if let Some(first_line) = modes.lines().next() {
                        if let Some((w_str, h_str)) = first_line.trim().split_once('x') {
                            if let (Ok(w), Ok(h)) = (w_str.parse::<u32>(), h_str.parse::<u32>()) {
                                width = w;
                                height = h;
                            }
                        }
                    }
                }

                monitors.push(Monitor {
                    id,
                    name,
                    width,
                    height,
                    x: 0,
                    y: 0,
                    scale: 1.0,
                    refresh_rate: 60.0,
                    focused: id == 0,
                });
                id += 1;
            }
        }
    }

    monitors
}
