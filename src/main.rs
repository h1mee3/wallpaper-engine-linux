mod cli;
mod compositor;
mod daemon;
mod discovery;
mod hyprland;
mod ipc;
mod noctalia;
mod pkg;
mod renderer;
mod vdf;
mod watcher;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, NoctaliaCommands, PkgCommands};
use daemon::Daemon;
use discovery::{Discovery, Wallpaper, WallpaperType};
use ipc::{IpcClient, IpcRequest, IpcResponse};
use noctalia::NoctaliaIntegration;
use pkg::PkgArchive;
use renderer::ScalingMode;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon => {
            tracing_subscriber::fmt()
                .with_env_filter("wengine=info")
                .init();
            println!("🚀 Starting Wallpaper Engine Linux daemon...");
            Daemon::run().await?;
        }
        Commands::List {
            json,
            filter_type,
            search,
        } => {
            let mut wallpapers = match IpcClient::send(IpcRequest::List).await {
                Ok(IpcResponse::Ok { data }) => {
                    serde_json::from_value::<Vec<Wallpaper>>(data).unwrap_or_default()
                }
                _ => Discovery::discover_all(&[]),
            };

            if let Some(ref ft) = filter_type {
                let target = ft.to_lowercase();
                wallpapers.retain(|w| match (&w.wallpaper_type, target.as_str()) {
                    (WallpaperType::Video, "video") => true,
                    (WallpaperType::Scene, "scene") => true,
                    (WallpaperType::Web, "web") => true,
                    _ => false,
                });
            }

            if let Some(ref q) = search {
                let q_lower = q.to_lowercase();
                wallpapers.retain(|w| {
                    w.title.to_lowercase().contains(&q_lower)
                        || w.tags.iter().any(|t| t.to_lowercase().contains(&q_lower))
                });
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&wallpapers)?);
            } else {
                println!(
                    "Found {} Wallpaper Engine wallpapers:\n",
                    wallpapers.len()
                );
                for (i, wp) in wallpapers.iter().enumerate() {
                    let type_badge = match wp.wallpaper_type {
                        WallpaperType::Video => "\x1b[34;1m[VIDEO]\x1b[0m",
                        WallpaperType::Scene => "\x1b[32;1m[SCENE]\x1b[0m",
                        WallpaperType::Web => "\x1b[33;1m[ WEB ]\x1b[0m",
                        _ => "\x1b[37;1m[OTHER]\x1b[0m",
                    };
                    println!(
                        "  \x1b[36m{:3}.\x1b[0m {} \x1b[1m{}\x1b[0m",
                        i + 1,
                        type_badge,
                        wp.title
                    );
                    println!(
                        "       \x1b[90mID:\x1b[0m {} | \x1b[90mFile:\x1b[0m {}",
                        wp.id, wp.file
                    );
                }
                println!("\nUse `wengine apply <ID>` to set a wallpaper.");
            }
        }
        Commands::Apply { query, monitor } => {
            match IpcClient::send(IpcRequest::Apply {
                id: query.clone(),
                monitor: monitor.clone(),
            })
            .await
            {
                Ok(IpcResponse::Ok { data }) => {
                    println!("✨ Wallpaper applied successfully!");
                    if let Some(title) = data.get("applied").and_then(|v| v.as_str()) {
                        println!("   Wallpaper: \x1b[1;32m{}\x1b[0m", title);
                    }
                    if let Some(mon) = data.get("monitor").and_then(|v| v.as_str()) {
                        println!("   Target:    \x1b[36m{}\x1b[0m", mon);
                    }
                }
                Ok(IpcResponse::Error { message }) => {
                    eprintln!("❌ Failed to apply wallpaper: {}", message);
                }
                Err(_) => {
                    eprintln!("⚠️  wengine daemon is not currently running.");
                    eprintln!("   Start the daemon with: `wengine daemon &`");
                }
            }
        }
        Commands::Scale { mode } => {
            let sm: ScalingMode = mode.into();
            match IpcClient::send(IpcRequest::Scaling { mode: sm }).await {
                Ok(IpcResponse::Ok { .. }) => {
                    println!("📐 Video display scaling mode set to: \x1b[1;32m{:?}\x1b[0m", sm);
                }
                _ => eprintln!("⚠️  wengine daemon is not running."),
            }
        }
        Commands::Status { json } => {
            match IpcClient::send(IpcRequest::Status).await {
                Ok(IpcResponse::Ok { data }) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&data)?);
                    } else {
                        println!("📊 \x1b[1mWallpaper Engine Status\x1b[0m");
                        let paused = data.get("paused").and_then(|v| v.as_bool()).unwrap_or(false);
                        let volume = data.get("volume").and_then(|v| v.as_u64()).unwrap_or(0);
                        let muted = data.get("muted").and_then(|v| v.as_bool()).unwrap_or(false);
                        let total = data.get("total_wallpapers").and_then(|v| v.as_u64()).unwrap_or(0);
                        let scaling = data.get("scaling_mode").and_then(|v| v.as_str()).unwrap_or("fill");

                        let de = data.get("desktop_environment").and_then(|v| v.as_str()).unwrap_or("Wayland");
                        let layer_shell = data.get("supports_layer_shell").and_then(|v| v.as_bool()).unwrap_or(true);

                        println!("   Desktop:   \x1b[34;1m{}\x1b[0m (Backend: {})", de, if layer_shell { "wlr-layer-shell" } else { "xdg-shell" });
                        println!("   Playback:  {}", if paused { "\x1b[33mPaused\x1b[0m" } else { "\x1b[32mPlaying\x1b[0m" });
                        println!("   Scaling:   \x1b[35;1m{}\x1b[0m (fill / fit / stretch)", scaling);
                        println!("   Volume:    {}% (Muted: {})", volume, muted);
                        println!("   Library:   {} wallpapers indexed", total);

                        if let Some(monitors) = data.get("monitors").and_then(|v| v.as_array()) {
                            println!("\n   \x1b[1mConnected Displays:\x1b[0m");
                            for m in monitors {
                                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let w = m.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                                let h = m.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                                let x = m.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
                                let y = m.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
                                let hz = m.get("refreshRate").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                println!("    🖥️  \x1b[36m{}\x1b[0m: {}x{} @ {:.0}Hz (+{}+{})", name, w, h, hz, x, y);
                            }
                        }

                        if let Some(active) = data.get("active_wallpapers").and_then(|v| v.as_object()) {
                            println!("\n   \x1b[1mActive Wallpapers:\x1b[0m");
                            for (mon, wp) in active {
                                let title = wp.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown");
                                let wtype = wp.get("wallpaper_type").and_then(|v| v.as_str()).unwrap_or("");
                                println!("    • \x1b[36m{}\x1b[0m: {} [{}]", mon, title, wtype);
                            }
                        }
                    }
                }
                Ok(IpcResponse::Error { message }) => eprintln!("❌ Error: {}", message),
                Err(_) => eprintln!("⚠️  wengine daemon is not currently running."),
            }
        }
        Commands::Toggle => {
            if let Ok(IpcResponse::Ok { data }) = IpcClient::send(IpcRequest::Toggle).await {
                let paused = data.get("paused").and_then(|v| v.as_bool()).unwrap_or(false);
                println!("Wallpaper playback {}", if paused { "paused ⏸️" } else { "resumed ▶️" });
            } else {
                eprintln!("⚠️  wengine daemon is not running.");
            }
        }
        Commands::Pause => {
            let _ = IpcClient::send(IpcRequest::Pause).await;
            println!("Wallpaper playback paused ⏸️");
        }
        Commands::Resume => {
            let _ = IpcClient::send(IpcRequest::Resume).await;
            println!("Wallpaper playback resumed ▶️");
        }
        Commands::Volume { level } => {
            let _ = IpcClient::send(IpcRequest::Volume { value: level }).await;
            println!("Volume set to {}%", level);
        }
        Commands::Mute { unmute } => {
            let _ = IpcClient::send(IpcRequest::Mute { value: !unmute }).await;
            println!("Wallpaper {}", if unmute { "unmuted 🔊" } else { "muted 🔇" });
        }
        Commands::Next { monitor } => {
            if let Ok(IpcResponse::Ok { data }) = IpcClient::send(IpcRequest::Next { monitor }).await {
                if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                    println!("Switched to next wallpaper: \x1b[1;32m{}\x1b[0m", title);
                }
            }
        }
        Commands::Prev { monitor } => {
            if let Ok(IpcResponse::Ok { data }) = IpcClient::send(IpcRequest::Prev { monitor }).await {
                if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                    println!("Switched to previous wallpaper: \x1b[1;32m{}\x1b[0m", title);
                }
            }
        }
        Commands::Random { monitor } => {
            if let Ok(IpcResponse::Ok { data }) = IpcClient::send(IpcRequest::Random { monitor }).await {
                if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                    println!("Switched to random wallpaper: \x1b[1;32m{}\x1b[0m", title);
                }
            }
        }
        Commands::Stop => {
            match IpcClient::send(IpcRequest::Stop).await {
                Ok(IpcResponse::Ok { .. }) => {
                    println!("🛑 Wallpaper Engine Linux daemon stopped successfully.");
                }
                _ => eprintln!("⚠️  wengine daemon is not currently running."),
            }
        }
        Commands::Steam => {
            println!("🎮 Launching official Wallpaper Engine via Steam...");
            let res = std::process::Command::new("steam")
                .arg("steam://rungameid/431960")
                .spawn();

            match res {
                Ok(_) => {
                    println!("✅ Sent launch command to Steam (App ID 431960).");
                    println!("💡 When you pick or change a wallpaper in the Steam app, `wengine` will automatically detect your selection and update your desktop!");
                }
                Err(e) => {
                    eprintln!("❌ Failed to launch Steam: {}", e);
                }
            }
        }
        Commands::Pkg { pkg_cmd } => match pkg_cmd {
            PkgCommands::List { path } => {
                let archive = PkgArchive::open(&path)?;
                println!("PKG Archive: {:?} ({}, {} files)", path, archive.magic, archive.entries.len());
                for (i, entry) in archive.entries.iter().enumerate() {
                    println!("  {:3}. {:<40} ({} bytes)", i + 1, entry.name, entry.length);
                }
            }
            PkgCommands::Extract { path, out } => {
                let out_dir = out.unwrap_or_else(|| {
                    path.parent().unwrap_or(&path).join("extracted")
                });
                let archive = PkgArchive::open(&path)?;
                println!("Extracting {} files to {:?}...", archive.entries.len(), out_dir);
                archive.extract_all(&out_dir)?;
                println!("✅ Extracted successfully!");
            }
        },
        Commands::Noctalia { noctalia_cmd } => match noctalia_cmd {
            NoctaliaCommands::Install => {
                let target_dir = NoctaliaIntegration::install()?;
                println!("✅ Successfully installed wengine plugin for Noctalia Shell!");
                println!("   Plugin directory: {:?}", target_dir);
                println!("\nTo add the widget to your Noctalia bar, edit `~/.config/noctalia/config.toml`:");
                println!("   start = [ \"workspaces\", \"wengine\", ... ]");
            }
        },
    }

    Ok(())
}
