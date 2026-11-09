use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::renderer::ScalingMode;

#[derive(Parser, Debug)]
#[command(
    name = "wengine",
    author = "hime",
    version = "0.2.0",
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

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the background daemon
    Daemon,

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
