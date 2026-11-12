use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopEnvironment {
    Hyprland,
    Sway,
    KdePlasma,
    Gnome,
    River,
    Wayfire,
    Labwc,
    Niri,
    Cosmic,
    GenericWayland,
    Unknown,
}

impl DesktopEnvironment {
    pub fn name(&self) -> &'static str {
        match self {
            DesktopEnvironment::Hyprland => "Hyprland",
            DesktopEnvironment::Sway => "Sway",
            DesktopEnvironment::KdePlasma => "KDE Plasma",
            DesktopEnvironment::Gnome => "GNOME",
            DesktopEnvironment::River => "River",
            DesktopEnvironment::Wayfire => "Wayfire",
            DesktopEnvironment::Labwc => "Labwc",
            DesktopEnvironment::Niri => "Niri",
            DesktopEnvironment::Cosmic => "COSMIC",
            DesktopEnvironment::GenericWayland => "Wayland (Generic)",
            DesktopEnvironment::Unknown => "Unknown",
        }
    }

    /// Whether the compositor is known to natively support `zwlr_layer_shell_v1`
    pub fn supports_layer_shell(&self) -> bool {
        match self {
            DesktopEnvironment::Hyprland
            | DesktopEnvironment::Sway
            | DesktopEnvironment::KdePlasma
            | DesktopEnvironment::River
            | DesktopEnvironment::Wayfire
            | DesktopEnvironment::Labwc
            | DesktopEnvironment::Niri
            | DesktopEnvironment::Cosmic => true,
            DesktopEnvironment::Gnome => false, // Mutter does not natively support wlr-layer-shell
            DesktopEnvironment::GenericWayland | DesktopEnvironment::Unknown => true,
        }
    }
}

/// Resolve XDG_RUNTIME_DIR, falling back to /run/user/<uid>
pub fn get_runtime_dir() -> Option<PathBuf> {
    if let Ok(dir) = env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(output) = Command::new("id").arg("-u").output() {
        if output.status.success() {
            let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let p = PathBuf::from(format!("/run/user/{}", uid));
            if p.exists() {
                env::set_var("XDG_RUNTIME_DIR", &p);
                return Some(p);
            }
        }
    }
    None
}

/// Find an active Wayland socket name (e.g. "wayland-0", "wayland-1") in XDG_RUNTIME_DIR
pub fn find_wayland_socket() -> Option<String> {
    let runtime_dir = get_runtime_dir()?;

    // If WAYLAND_DISPLAY is already set and exists, keep it
    if let Ok(wd) = env::var("WAYLAND_DISPLAY") {
        if !wd.is_empty() && runtime_dir.join(&wd).exists() {
            return Some(wd);
        }
    }

    // Prefer wayland-1, wayland-0, or any wayland-* socket (not .lock)
    for preferred in ["wayland-1", "wayland-0"] {
        let p = runtime_dir.join(preferred);
        if p.exists() && !preferred.ends_with(".lock") {
            return Some(preferred.to_string());
        }
    }

    if let Ok(entries) = std::fs::read_dir(&runtime_dir) {
        let mut candidates: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("wayland-") && !name.ends_with(".lock") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        candidates.sort();
        if let Some(first) = candidates.into_iter().next() {
            return Some(first);
        }
    }

    None
}

/// Import missing Wayland/DE environment variables from systemd --user environment if available
fn import_systemd_env() {
    let keys = [
        "WAYLAND_DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "XDG_SESSION_TYPE",
        "HYPRLAND_INSTANCE_SIGNATURE",
        "SWAYSOCK",
        "NIRI_SOCKET",
        "RIVERSOCK",
        "KDE_FULL_SESSION",
        "KDE_SESSION_VERSION",
    ];

    let any_missing = keys.iter().any(|k| env::var(k).is_err());
    if !any_missing {
        return;
    }

    if let Ok(output) = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if keys.contains(&k) && env::var(k).is_err() && !v.is_empty() {
                        debug!("Imported {}={} from systemd user environment", k, v);
                        env::set_var(k, v);
                    }
                }
            }
        }
    }
}

/// Ensure WAYLAND_DISPLAY and session env vars are populated synchronously
pub fn ensure_wayland_env() -> bool {
    import_systemd_env();
    if let Some(sock) = find_wayland_socket() {
        if env::var("WAYLAND_DISPLAY").ok().as_deref() != Some(sock.as_str()) {
            info!("Detected Wayland display socket: {}", sock);
            env::set_var("WAYLAND_DISPLAY", &sock);
        }
        return true;
    }
    false
}

/// Wait up to 10s for a Wayland display socket to appear (handles display manager / SDDM / GDM login race)
pub async fn wait_for_wayland_env() -> bool {
    if ensure_wayland_env() {
        return true;
    }

    info!("Waiting for Wayland display socket to become available...");
    for _ in 0..50 {
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        if ensure_wayland_env() {
            return true;
        }
    }

    warn!("Timed out waiting for Wayland display socket; proceeding with fallbacks");
    false
}

/// Detect the active desktop environment / window manager
pub fn detect_desktop_environment() -> DesktopEnvironment {
    // 1. Primary standard: Check XDG Desktop variables (set by display manager / session)
    let current_desktop = env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| env::var("XDG_SESSION_DESKTOP"))
        .or_else(|_| env::var("DESKTOP_SESSION"))
        .unwrap_or_default()
        .to_lowercase();

    if current_desktop.contains("kde") || current_desktop.contains("plasma") {
        return DesktopEnvironment::KdePlasma;
    }
    if current_desktop.contains("hyprland") {
        return DesktopEnvironment::Hyprland;
    }
    if current_desktop.contains("sway") {
        return DesktopEnvironment::Sway;
    }
    if current_desktop.contains("gnome") {
        return DesktopEnvironment::Gnome;
    }
    if current_desktop.contains("river") {
        return DesktopEnvironment::River;
    }
    if current_desktop.contains("wayfire") {
        return DesktopEnvironment::Wayfire;
    }
    if current_desktop.contains("labwc") {
        return DesktopEnvironment::Labwc;
    }
    if current_desktop.contains("niri") {
        return DesktopEnvironment::Niri;
    }
    if current_desktop.contains("cosmic") {
        return DesktopEnvironment::Cosmic;
    }

    // 2. Secondary check: Environment indicators with socket validation
    if env::var("KDE_FULL_SESSION").is_ok() || env::var("KDE_SESSION_VERSION").is_ok() {
        return DesktopEnvironment::KdePlasma;
    }

    if let Ok(sig) = env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
            let sock = PathBuf::from(runtime_dir).join("hypr").join(&sig).join(".socket.sock");
            if sock.exists() {
                return DesktopEnvironment::Hyprland;
            }
        } else {
            return DesktopEnvironment::Hyprland;
        }
    }

    if let Ok(swaysock) = env::var("SWAYSOCK") {
        if PathBuf::from(&swaysock).exists() {
            return DesktopEnvironment::Sway;
        }
    }

    if let Ok(riversock) = env::var("RIVERSOCK") {
        if PathBuf::from(&riversock).exists() {
            return DesktopEnvironment::River;
        }
    }

    if env::var("NIRI_SOCKET").is_ok() {
        return DesktopEnvironment::Niri;
    }

    if env::var("WAYFIRE_CONFIG_FILE").is_ok() {
        return DesktopEnvironment::Wayfire;
    }

    if env::var("GNOME_DESKTOP_SESSION_ID").is_ok() {
        return DesktopEnvironment::Gnome;
    }

    // 3. Probe runtime directory sockets if launched outside session env (e.g. systemd/SDDM)
    if let Some(r) = get_runtime_dir() {
        if r.join("hypr").is_dir() {
            if let Ok(entries) = std::fs::read_dir(r.join("hypr")) {
                for entry in entries.flatten() {
                    if entry.path().join(".socket.sock").exists() {
                        return DesktopEnvironment::Hyprland;
                    }
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(&r) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.starts_with("sway-ipc.") && fname.ends_with(".sock") {
                    return DesktopEnvironment::Sway;
                }
                if fname.starts_with("niri.wayland-") && fname.ends_with(".sock") {
                    return DesktopEnvironment::Niri;
                }
            }
        }
    }

    // 4. Check if generic Wayland session is active
    if env::var("WAYLAND_DISPLAY").is_ok()
        || env::var("XDG_SESSION_TYPE").map(|s| s.to_lowercase() == "wayland").unwrap_or(false)
        || find_wayland_socket().is_some()
    {
        return DesktopEnvironment::GenericWayland;
    }

    DesktopEnvironment::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detection() {
        let de = detect_desktop_environment();
        println!("Detected desktop environment: {}", de.name());
        assert_ne!(de.name(), "");
    }
}

