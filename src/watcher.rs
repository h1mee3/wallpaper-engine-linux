use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::daemon::DaemonState;
use crate::discovery::Discovery;

pub struct SteamWatcher;

impl SteamWatcher {
    /// Find potential config.json and appworkshop_431960.acf locations
    pub fn get_watch_targets() -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut config_files = Vec::new();
        let mut workshop_files = Vec::new();

        let libs = Discovery::find_steam_libraries();
        for lib in libs {
            // Official Wallpaper Engine config.json
            let cfg = lib.join("steamapps/common/wallpaper_engine/config.json");
            config_files.push(cfg);

            // Proton prefix configs
            let pfx = lib.join("steamapps/compatdata/431960/pfx/drive_c/users");
            if let Ok(users) = fs::read_dir(&pfx) {
                for u in users.flatten() {
                    let path1 = u.path().join("AppData/Local/WallpaperEngine/config.json");
                    let path2 = u.path().join("Application Data/WallpaperEngine/config.json");
                    config_files.push(path1);
                    config_files.push(path2);
                }
            }

            // Workshop ACF file (updated when subscribing in Steam client)
            let acf = lib.join("steamapps/workshop/appworkshop_431960.acf");
            workshop_files.push(acf);
        }

        (config_files, workshop_files)
    }

    /// Convert a Windows/Wine path into a Linux path
    pub fn normalize_wine_path(win_path: &str) -> PathBuf {
        let mut s = win_path.replace('\\', "/");
        if s.len() >= 2 && s.chars().nth(1) == Some(':') {
            let drive = s.chars().next().unwrap().to_ascii_uppercase();
            if drive == 'Z' {
                s = s[2..].to_string();
            }
        }
        PathBuf::from(s)
    }

    /// Extract wallpaper paths from Wallpaper Engine's config.json
    pub fn parse_config_json(content: &str) -> Vec<(String, PathBuf)> {
        let mut results = Vec::new();
        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(_) => return results,
        };

        // Format 1: "selectedwallpapers": { "DISPLAY1": { "file": "..." } }
        if let Some(selected) = parsed.get("selectedwallpapers").and_then(|v| v.as_object()) {
            for (mon, val) in selected {
                if let Some(file_str) = val.get("file").and_then(|v| v.as_str()) {
                    let path = Self::normalize_wine_path(file_str);
                    results.push((mon.clone(), path));
                }
            }
        }

        // Format 2: "monitors": [ { "selectedwallpaper": "..." } ]
        if let Some(monitors) = parsed.get("monitors").and_then(|v| v.as_array()) {
            for (idx, mon) in monitors.iter().enumerate() {
                if let Some(file_str) = mon.get("selectedwallpaper").and_then(|v| v.as_str()) {
                    let path = Self::normalize_wine_path(file_str);
                    results.push((format!("monitor{}", idx), path));
                }
            }
        }

        results
    }

    /// Start the background watcher loop
    pub fn start(state: Arc<Mutex<DaemonState>>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut mtimes: HashMap<PathBuf, SystemTime> = HashMap::new();
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(1000));
            let mut initialized = false;

            debug!("Steam and config.json watcher started");

            loop {
                interval.tick().await;

                let (config_targets, workshop_targets) = Self::get_watch_targets();

                // 1. Check for Steam Workshop updates (new subscriptions)
                for acf in workshop_targets {
                    if let Ok(meta) = fs::metadata(&acf) {
                        if let Ok(mtime) = meta.modified() {
                            let prev = mtimes.get(&acf).copied();
                            if initialized && prev.map_or(false, |p| mtime > p) {
                                info!("Detected Steam Workshop changes in {:?}", acf);
                                let mut s = state.lock().await;
                                s.rescan();
                            }
                            mtimes.insert(acf, mtime);
                        }
                    }
                }

                // 2. Check for Wallpaper Engine config.json changes (selected in official UI)
                for cfg in config_targets {
                    if let Ok(meta) = fs::metadata(&cfg) {
                        if let Ok(mtime) = meta.modified() {
                            let prev = mtimes.get(&cfg).copied();
                            let should_trigger = if initialized {
                                prev.map_or(true, |p| mtime > p)
                            } else {
                                // On first startup, read existing config.json
                                true
                            };

                            if should_trigger {
                                info!("Reading Wallpaper Engine config from {:?}", cfg);
                                if let Ok(content) = fs::read_to_string(&cfg) {
                                    let selections = Self::parse_config_json(&content);
                                    for (_mon, path) in selections {
                                        info!("Steam app selected wallpaper: {:?}", path);
                                        let id_opt = path
                                            .parent()
                                            .and_then(|p| p.file_name())
                                            .map(|s| s.to_string_lossy().to_string())
                                            .or_else(|| {
                                                path.file_name()
                                                    .map(|s| s.to_string_lossy().to_string())
                                            });

                                        if let Some(id) = id_opt {
                                            let mut s = state.lock().await;
                                            let _ = s.apply_by_query(&id, None).await;
                                        }
                                    }
                                }
                            }
                            mtimes.insert(cfg, mtime);
                        }
                    }
                }

                initialized = true;
            }
        })
    }

}
