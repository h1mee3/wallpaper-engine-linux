use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

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

/// Detect the active desktop environment / window manager
pub fn detect_desktop_environment() -> DesktopEnvironment {
    // 1. Check Hyprland
    if env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        return DesktopEnvironment::Hyprland;
    }

    // 2. Check Sway
    if env::var("SWAYSOCK").is_ok() {
        return DesktopEnvironment::Sway;
    }

    // 3. Check River
    if env::var("RIVERSOCK").is_ok() {
        return DesktopEnvironment::River;
    }

    // 4. Check Niri
    if env::var("NIRI_SOCKET").is_ok() {
        return DesktopEnvironment::Niri;
    }

    // 5. Check Wayfire
    if env::var("WAYFIRE_CONFIG_FILE").is_ok() {
        return DesktopEnvironment::Wayfire;
    }

    // 6. Check KDE Plasma
    if env::var("KDE_FULL_SESSION").is_ok() {
        return DesktopEnvironment::KdePlasma;
    }

    // 7. Check GNOME
    if env::var("GNOME_DESKTOP_SESSION_ID").is_ok() {
        return DesktopEnvironment::Gnome;
    }

    // 8. Check XDG environment variables
    let current_desktop = env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| env::var("XDG_SESSION_DESKTOP"))
        .or_else(|_| env::var("DESKTOP_SESSION"))
        .unwrap_or_default()
        .to_lowercase();

    if current_desktop.contains("hyprland") {
        return DesktopEnvironment::Hyprland;
    }
    if current_desktop.contains("sway") {
        return DesktopEnvironment::Sway;
    }
    if current_desktop.contains("kde") || current_desktop.contains("plasma") {
        return DesktopEnvironment::KdePlasma;
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

    // 9. Check if generic Wayland session is active
    if env::var("WAYLAND_DISPLAY").is_ok()
        || env::var("XDG_SESSION_TYPE").map(|s| s.to_lowercase() == "wayland").unwrap_or(false)
    {
        return DesktopEnvironment::GenericWayland;
    }

    // 10. Check for default wayland socket in XDG_RUNTIME_DIR
    if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
        if PathBuf::from(runtime_dir).join("wayland-0").exists() {
            return DesktopEnvironment::GenericWayland;
        }
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

