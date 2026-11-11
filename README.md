# 🌌 wengine: Wallpaper Engine Linux Layer

A high-performance, native Linux layer written in **Rust** to run **Steam Wallpaper Engine** wallpapers natively on **any Wayland Desktop Environment and Window Manager** (Hyprland, Sway, KDE Plasma, GNOME, River, Wayfire, Labwc, Niri, COSMIC, etc.).

---

## 🎬 Demo

<p align="center">
  <video src="assets/demo.mp4" controls width="100%" poster="assets/demo-preview.png"></video>
  <br>
  <em>▶️ <b>Interactive & Video Wallpapers on Hyprland / Wayland:</b> <a href="assets/demo.mp4">assets/demo.mp4</a></em>
</p>

---

## ✨ Features

- **🦀 Pure Rust Core & Daemon**: Blazing fast, zero memory footprint, rock-solid asynchronous daemon built with Tokio and Unix Domain Sockets.
- **🌐 Universal Wayland Compositor Support**:
  - Works out-of-the-box on **any** Wayland DE or WM:
    - **Hyprland** (native `socket2` event loop & layer rules)
    - **Sway** (native IPC event stream & output management)
    - **KDE Plasma 6 / 5.24+** (KWin native layer shell & DBus palette sync)
    - **GNOME / Mutter** (dual-mode: native via `wlr-layer-shell` extension or automatic `xdg_wm_base` fullscreen fallback)
    - **River**, **Wayfire**, **Labwc**, **Niri**, **COSMIC**, **DWL**, and generic Wayland sessions.
- **🖼️ Dual-Backend Wayland Render Engine (`wengine-layer-player`)**:
  - **`wlr-layer-shell` Backend**: Operates strictly on `Layer level 0 (background)` via `zwlr_layer_surface_v1`. True compositor desktop background rendered behind all tiled windows, floating windows, and desktop widgets with zero window rules or opacity hacks.
  - **`xdg-shell` Fallback Backend**: Automatically falls back to borderless `xdg_wm_base` on compositors that do not implement layer-shell (such as stock GNOME/Mutter), ensuring seamless rendering everywhere.
  - **Multi-Monitor Targeted Geometry**: Directly tracks and binds specific Wayland display outputs (`DP-1`, `HDMI-A-1`, `eDP-1`, etc.) with exact display geometry and resolution fitting (`1920x1080 @ 200Hz`), supporting `fill`, `fit`, and `stretch` scaling modes.
- **🖥️ Multi-Tier Display & Output Discovery**:
  - **Tier 1 (Native Wayland Protocol)**: Pure Wayland client (`wl_output` v4) queries connected displays, resolutions, positions, refresh rates, and scales directly from the compositor with zero external CLI dependencies.
  - **Tier 2 (Compositor IPC)**: Hyprland (`hyprctl`), Sway (`swaymsg`), KDE (`kscreen-doctor`), and `wlr-randr`.
  - **Tier 3 (Linux DRM Hardware)**: Direct kernel DRM subsystem probing (`/sys/class/drm/`) for connected status and native display modes.
- **🎮 Live Steam App Synchronization (`SteamWatcher`)**:
  - Live inotify watcher tracking Steam's Wallpaper Engine `config.json` and `appworkshop_431960.acf`.
  - Pick or change wallpapers directly inside the official Steam Wallpaper Engine application GUI or Workshop; `wengine` detects the selection instantly and applies it in real-time.
- **🔍 Steam & Workshop Auto-Discovery**:
  - Automatically parses Valve's VDF (`libraryfolders.vdf`, `appmanifest_431960.acf`).
  - Scans Steam libraries across all storage drives (`~/.local/share/Steam`, `/hdd/SteamLibrary`, `/run/media/...`).
  - Discovers official Wallpaper Engine default projects (`Neon Sunset`, `Techno`, `Beach`, etc.) and downloaded Steam Workshop items (`workshop/content/431960/`).
  - Supports custom user wallpaper directories.
- **🎬 Hardware-Accelerated Video Wallpapers**:
  - Direct hardware-accelerated decoding (VA-API / NVDEC / Vulkan) supporting 4K, 60fps, 144fps, and 200Hz displays.
  - Gapless, instant wallpaper switching over IPC without restarting processes.
  - Full volume and mute controls.
- **✨ Interactive & Reactive Web Wallpapers (`wengine-web-player`)**:
  - **Strictly Zero Electron**: Built in pure native C using WebKitGTK 4.1 and `gtk-layer-shell` (28KB binary, ultra-optimized, minimal memory footprint).
  - **Full Desktop Interactivity**: Mouse click, drag, spring velocity, and slingshot physics (e.g. Workshop item `2856406172` *Lycoris Recoil / Sakana*).
  - **Wayland Layer Integration**: Anchored to Layer Shell `BOTTOM`, placed directly behind open windows but above static backgrounds so you can interact with desktop characters seamlessly.
  - **Transparent Canvas & Widget Support**: Auto-detects alpha transparency modes and hides web chrome, seamlessly blending interactive characters onto your desktop.
- **📦 Native `.pkg` Scene Archive Unpacker**:
  - Pure Rust binary parser for Wallpaper Engine `scene.pkg` archives (`PKGV0001`, `PKGV0016`, `PKGV0021`, etc.).
  - Extracts and inspects `scene.json`, particle systems, shaders, textures, and embedded audio.
- **🌱 Configurable Power-Save & Smart Auto-Pause Modes**:
  - **`always`**: Keep running continuously in the background.
  - **`fullscreen`** *(default)*: Pauses when a game or app is in fullscreen mode.
  - **`power-save`** (`covered`): Smart app detection! Auto-pauses wallpaper playback whenever application windows are above the wallpaper on the active workspace to save **100% CPU and GPU resources**. Automatically resumes as soon as you switch to an empty workspace or minimize windows!
  - Toggle anytime via `wengine power-save on` or `wengine pause-mode power-save`.
- **🎨 Cross-Desktop Wallpaper Synchronization**:
  - Automatically synchronizes wallpaper images/previews with:
    - **GNOME** (via `gsettings` dark and light picture URI)
    - **KDE Plasma** (via DBus JavaScript evaluation)
    - **Sway** (via `swaymsg output bg`)
    - **Hyprland** (via `hyprctl hyprpaper`)
    - **Noctalia Shell** (via Luau plugin & dynamic color palette sync)
    - **swww** (via `swww img`)

---

## 🚀 Quick Start

### 1. Build & Install CLI

```bash
cargo build --release
ln -sf $(pwd)/target/release/wengine ~/.local/bin/wengine
```

### 2. Start the Daemon

Start in background or via systemd:
```bash
wengine daemon &
```

Or enable the systemd user service:
```bash
systemctl --user enable --now wengine
```

### 3. List & Apply Wallpapers

```bash
# Check status and active desktop environment
wengine status

# List all discovered wallpapers
wengine list

# Filter only 4K/60fps video wallpapers
wengine list --type video

# Apply a wallpaper by Workshop ID or title
wengine apply 969834539 --monitor DP-1
wengine apply 2856406172               # Interactive Sakana / Lycoris Recoil widget!
wengine apply "Raining in Tokyo"

# Control playback & volume
wengine toggle
wengine next
wengine random
wengine volume 40
wengine mute

# Power-save & pause policy customization
wengine power-save on                  # Auto-pause when apps are above the wallpaper
wengine power-save off                 # Return to standard fullscreen pause
wengine pause-mode always              # Never pause (keep running in background)
wengine pause-mode fullscreen          # Standard gaming pause
wengine pause-mode power-save          # Max battery & GPU savings
```

### 4. Install Noctalia Shell Plugin (Optional)

```bash
wengine noctalia install
```

Add the widget to your Noctalia bar in `~/.config/noctalia/config.toml`:
```toml
[bar.default]
start = [
  "workspaces",
  "wengine",
  "sysmon"
]
```

### 5. Inspect / Extract `.pkg` Scene Files

```bash
# Inspect contents of a scene archive
wengine pkg list /path/to/scene.pkg

# Extract all textures, shaders, models, and scene.json
wengine pkg extract /path/to/scene.pkg --out ./extracted/
```

---

## 🛠️ Architecture

```
┌────────────────────────────────────────────────────────┐
│                      wengine CLI                       │
└──────────────────────────┬─────────────────────────────┘
                           │ Unix Domain Socket IPC
┌──────────────────────────▼─────────────────────────────┐
│                     wengine Daemon                     │
│  - Library Discovery (Steam VDF + Workshop 431960)     │
│  - PKG Archive Engine (PKGV0001 - PKGV0021)            │
│  - Smart Auto-Pause & Power-Save Policy Manager        │
│  - Universal Wayland Display Discovery (wl_output)     │
└──────┬───────────────────┬──────────────────────┬──────┘
       │                   │                      │
┌──────▼────────────┐┌─────▼──────────────┐┌──────▼─────────────┐
│ Compositor Bridge ││    Render Host     ││ Desktop Sync Engine│
│ - Hyprland socket2││- Video Player (mpv ││ - GNOME (gsettings)│
│ - Sway IPC        ││  wlr-layer-shell)  ││ - KDE Plasma (DBus)│
│ - Workspace & App ││- Web Player (native││ - Noctalia Plugin  │
│   Layer Detection ││  WebKitGTK, zero   ││ - Sway / swww      │
│ (Power-Save Loop) ││  Electron, 28KB)   ││                    │
└───────────────────┘└────────────────────┘└────────────────────┘
```

---

## 📜 License
MIT
