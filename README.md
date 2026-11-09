# 🌌 wengine: Wallpaper Engine Linux Layer

A high-performance, native Linux layer written in **Rust** to run **Steam Wallpaper Engine** wallpapers natively on **Hyprland**, **Noctalia**, and **Wayland**.

---

## ✨ Features

- **🦀 Pure Rust Core & Daemon**: Blazing fast, zero memory footprint, rock-solid asynchronous daemon built with Tokio and Unix Domain Sockets.
- **🖼️ Native Wayland `wlr-layer-shell` Backend**:
  - Operates strictly on `Layer level 0 (background)` via `zwlr_layer_surface_v1`.
  - True compositor desktop background: rendered completely behind all tiled windows, floating windows, and desktop widgets.
  - Zero window rules or opacity hacks needed; user applications enjoy 100% unobstructed visibility and full input focus.
  - Exact display geometry and resolution fitting (`1920x1080 @ 200Hz`), supporting `fill`, `fit`, and `stretch` scaling modes.
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
- **📦 Native `.pkg` Scene Archive Unpacker**:
  - Pure Rust binary parser for Wallpaper Engine `scene.pkg` archives (`PKGV0001`, `PKGV0016`, `PKGV0021`, etc.).
  - Extracts and inspects `scene.json`, particle systems, shaders, textures, and embedded audio.
- **⚡ Native Hyprland Integration**:
  - Connects to Hyprland's `socket2` event stream.
  - **Auto-pause on Fullscreen**: Automatically pauses video/scene rendering when launching a game or entering fullscreen (`fullscreen>>1`) to free 100% of GPU/CPU for your game!
  - **Auto-resume on Exit**: Resumes smoothly as soon as you exit fullscreen.
  - Multi-monitor support: query and assign wallpapers per output (e.g. `DP-1`) or across all monitors.
- **🌺 Native Noctalia Shell Integration**:
  - Bundled Luau plugin for Noctalia v5+ (`widget.luau`, `panel.luau`, `service.luau`).
  - **Bar Widget**: Icon in the Noctalia capsule bar showing live playback status (click to open picker, right-click to pause/resume).
  - **Interactive Picker Panel**: Browse thumbnails, search by title/tags, filter by type (Video vs Scene), and apply with one click.
  - **Dynamic Theme Palette Sync**: Automatically triggers Noctalia wallpaper color palette generation to sync the desktop theme with the active wallpaper.

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
# List all discovered wallpapers
wengine list

# Filter only 4K/60fps video wallpapers
wengine list --type video

# Apply a wallpaper by Workshop ID or title
wengine apply 969834539 --monitor DP-1
wengine apply "Raining in Tokyo"

# Control playback
wengine toggle
wengine next
wengine random
wengine volume 40
wengine mute
```

### 4. Install Noctalia Shell Plugin

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
│  - State & Playlist Coordinator                        │
└──────┬───────────────────┬──────────────────────┬──────┘
       │                   │                      │
┌──────▼──────┐     ┌──────▼──────┐        ┌──────▼──────┐
│  Hyprland   │     │ Render Host │        │  Noctalia   │
│   Socket2   │     │ (MPV / EGL  │        │ Shell Plugin│
│ Event Loop  │     │ Layer-Shell)│        │  (Luau API) │
│ (Auto-Pause)│     │             │        │             │
└─────────────┘     └─────────────┘        └─────────────┘
```

---

## 📜 License
MIT
