use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::renderer::ScalingMode;

#[derive(Parser, Debug)]
#[command(
    name = "wengine",
    author = "hime",
    version = env!("CARGO_PKG_VERSION"),
    about = "Wallpaper Engine Linux Layer for any Wayland DE/WM (Hyprland, Sway, KDE, GNOME, River, Niri, Cosmic, etc.)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliScalingMode {
    /// Crop / zoom to fill entire display resolution with zero black bars (Default)
    Fill,
    /// Letterbox / pillarbox to show 100% of video without cropping
    Fit,
    /// Stretch video to exact display aspect ratio
    Stretch,
}

impl From<CliScalingMode> for ScalingMode {
    fn from(c: CliScalingMode) -> Self {
        match c {
            CliScalingMode::Fill => ScalingMode::Fill,
            CliScalingMode::Fit => ScalingMode::Fit,
            CliScalingMode::Stretch => ScalingMode::Stretch,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliPauseMode {
    /// Keep running in background continuously (never auto-pause)
    Always,
    /// Pause only when an application is fullscreen (Default)
    Fullscreen,
    /// Pause whenever an application window is above the wallpaper / covering active workspace
    Covered,
    /// Synonym for Covered: maximum power saving, only run when desktop is visible
    PowerSave,
}

impl From<CliPauseMode> for crate::ipc::PauseMode {
    fn from(c: CliPauseMode) -> Self {
        match c {
            CliPauseMode::Always => crate::ipc::PauseMode::Always,
            CliPauseMode::Fullscreen => crate::ipc::PauseMode::Fullscreen,
            CliPauseMode::Covered | CliPauseMode::PowerSave => crate::ipc::PauseMode::PowerSave,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the background daemon
    Daemon {
        /// Enable power-save mode (auto-pauses wallpaper whenever an app is above the wallpaper)
        #[arg(long)]
        power_save: bool,

        /// Pause policy mode: always, fullscreen, covered, power-save
        #[arg(long)]
        pause_mode: Option<CliPauseMode>,

        /// Maximum video wallpaper FPS (0 = unlimited, default: 30)
        #[arg(long)]
        fps: Option<u32>,
    },

    /// List all discovered Wallpaper Engine wallpapers
    List {
        /// Output results as JSON
        #[arg(long)]
        json: bool,

        /// Filter by wallpaper type: video, scene, web
        #[arg(short = 't', long = "type")]
        filter_type: Option<String>,

        /// Filter by search keyword
        #[arg(short = 's', long = "search")]
        search: Option<String>,
    },

    /// Apply a wallpaper by ID or title to a monitor
    Apply {
        /// Wallpaper ID (folder name / workshop ID) or title substring
        query: String,

        /// Target monitor (e.g. DP-1, HDMI-A-1, or '*' for all)
        #[arg(short = 'm', long = "monitor")]
        monitor: Option<String>,
    },

    /// Show daemon and playback status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Set video display scaling mode (fill / fit / stretch)
    Scale {
        /// Scaling mode: fill (crop to fill display, no black bars), fit, stretch
        mode: CliScalingMode,
    },

    /// Set maximum video wallpaper FPS (0 = unlimited, default: 30)
    Fps {
        /// Target FPS limit (e.g. 30, 60, or 0 for unlimited)
        fps: u32,
    },

    /// Set auto-pause and power-saving policy mode (always, fullscreen, power-save)
    PauseMode {
        /// Policy mode: always (keep running), fullscreen (pause on fullscreen), power-save (pause when covered by apps)
        mode: CliPauseMode,
    },

    /// Toggle or set power-save mode (pauses wallpaper when apps are above it)
    PowerSave {
        /// Enable or disable power-save mode (on/off, true/false, 1/0)
        #[arg(value_name = "on/off")]
        state: Option<String>,
    },

    /// Toggle wallpaper pause/playback
    Toggle,

    /// Pause wallpaper playback
    Pause,

    /// Resume wallpaper playback
    Resume,

    /// Set wallpaper volume (0 - 100)
    Volume {
        /// Volume level percentage
        level: u32,
    },

    /// Mute wallpaper audio
    Mute {
        /// Unmute instead of muting
        #[arg(long)]
        unmute: bool,
    },

    /// Switch to next wallpaper
    Next {
        #[arg(short = 'm', long = "monitor")]
        monitor: Option<String>,
    },

    /// Switch to previous wallpaper
    Prev {
        #[arg(short = 'm', long = "monitor")]
        monitor: Option<String>,
    },

    /// Switch to a random wallpaper
    Random {
        #[arg(short = 'm', long = "monitor")]
        monitor: Option<String>,
    },

    /// Stop the background daemon and active wallpaper processes
    Stop,

    /// Open the graphical Wallpaper Engine selector UI
    Ui {
        /// Open in your default web browser instead of the native window
        #[arg(long)]
        web: bool,
    },

    /// Sync and apply the wallpaper currently selected in Steam's Wallpaper Engine
    SyncSteam,

    /// Check for and install prebuilt updates from GitHub Releases (no cargo rebuild needed)
    Update {
        /// Only check if an update is available without installing
        #[arg(long)]
        check: bool,
    },

    /// Launch the official Wallpaper Engine app via Steam
    Steam,

    /// Inspect and extract Wallpaper Engine .pkg scene archives
    Pkg {
        #[command(subcommand)]
        pkg_cmd: PkgCommands,
    },

    /// Manage Noctalia Shell integration
    Noctalia {
        #[command(subcommand)]
        noctalia_cmd: NoctaliaCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum PkgCommands {
    /// List entries inside a .pkg archive
    List {
        /// Path to scene.pkg
        path: PathBuf,
    },
    /// Extract all files from a .pkg archive
    Extract {
        /// Path to scene.pkg
        path: PathBuf,
        /// Destination directory
        #[arg(short = 'o', long = "out")]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum NoctaliaCommands {
    /// Install wengine plugin into Noctalia Shell
    Install,
}
