use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::daemon::DaemonState;
use crate::discovery::Discovery;

pub struct SteamWatcher;

impl SteamWatcher {
    /// Find potential config.json and appworkshop_431960.acf locations
    pub fn get_watch_targets() -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut config_files = Vec::new();
        let mut workshop_files = Vec::new();

        let mut seen = std::collections::HashSet::new();
        let libs = Discovery::find_steam_libraries();

        for lib in libs {
            if !lib.exists() {
                continue;
            }

            // Canonicalize to deduplicate symlinks / mount points
            let canon = lib.canonicalize().unwrap_or(lib);
            if !seen.insert(canon.clone()) {
                continue;
            }

            // Official Wallpaper Engine config.json
            let cfg = canon.join("steamapps/common/wallpaper_engine/config.json");
            config_files.push(cfg);

            // Proton prefix configs
            let pfx = canon.join("steamapps/compatdata/431960/pfx/drive_c/users");
            if let Ok(users) = fs::read_dir(&pfx) {
                for u in users.flatten() {
                    let path1 = u.path().join("AppData/Local/WallpaperEngine/config.json");
                    let path2 = u.path().join("Application Data/WallpaperEngine/config.json");
                    config_files.push(path1);
                    config_files.push(path2);
                }
            }

            // Workshop ACF file (updated when subscribing in Steam client)
            let acf = canon.join("steamapps/workshop/appworkshop_431960.acf");
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

        fn collect_selected(
            obj: &serde_json::Map<String, Value>,
            results: &mut Vec<(String, PathBuf)>,
        ) {
            for (mon, val) in obj {
                if let Some(file_str) = val.get("file").and_then(|v| v.as_str()) {
                    if !file_str.trim().is_empty() {
                        let path = SteamWatcher::normalize_wine_path(file_str);
                        results.push((mon.clone(), path));
                    }
                }
            }
        }

        // Format 1: Top-level "selectedwallpapers": { "//./DISPLAY1": { "file": "..." } }
        if let Some(selected) = parsed.get("selectedwallpapers").and_then(|v| v.as_object()) {
            collect_selected(selected, &mut results);
        }

        // Format 2: "steamuser.general.wallpaperconfig.selectedwallpapers"
        if results.is_empty() {
            if let Some(selected) = parsed
                .pointer("/steamuser/general/wallpaperconfig/selectedwallpapers")
                .and_then(|v| v.as_object())
            {
                collect_selected(selected, &mut results);
            }
        }

        // Format 3: "monitors": [ { "selectedwallpaper": "..." } ]
        if let Some(monitors) = parsed.get("monitors").and_then(|v| v.as_array()) {
            for (idx, mon) in monitors.iter().enumerate() {
                if let Some(file_str) = mon.get("selectedwallpaper").and_then(|v| v.as_str()) {
                    if !file_str.trim().is_empty() {
                        let path = Self::normalize_wine_path(file_str);
                        results.push((format!("monitor{}", idx), path));
                    }
                }
            }
        }

        results
    }

    /// Extract wallpaper ID (folder name) from a Wallpaper Engine file or directory path
    pub fn extract_wallpaper_id(path: &std::path::Path) -> Option<String> {
        let has_ext = path.extension().is_some();
        let is_project_json = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("project.json"))
            .unwrap_or(false);

        if is_project_json || has_ext {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
        } else {
            path.file_name()
                .map(|s| s.to_string_lossy().to_string())
        }
    }

    /// Map a Steam Wallpaper Engine display key (e.g. "//./DISPLAY1", "monitor0") to a Wayland monitor
    pub fn map_steam_monitor(steam_mon: &str, monitors: &[crate::compositor::Monitor]) -> Option<String> {
        if monitors.len() <= 1 {
            return None;
        }
        let lower = steam_mon.to_ascii_lowercase();
        // Extract trailing digits (e.g. DISPLAY1 -> 1-based, monitor0 -> 0-based)
        let digits: String = lower.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>().chars().rev().collect();
        if let Ok(num) = digits.parse::<usize>() {
            let idx = if lower.contains("display") && num > 0 {
                num - 1
            } else {
                num
            };
            return monitors.get(idx).map(|m| m.name.clone());
        }
        None
    }

    /// Read all currently selected wallpapers from Steam Wallpaper Engine's config.json
    pub fn read_current_selections() -> Vec<(String, PathBuf)> {
        let (config_targets, _) = Self::get_watch_targets();
        let mut all = Vec::new();
        for cfg in config_targets {
            if let Ok(content) = fs::read_to_string(&cfg) {
                let selections = Self::parse_config_json(&content);
                if !selections.is_empty() {
                    all.extend(selections);
                }
            }
        }
        all
    }

    /// Start the background watcher loop
    pub fn start(state: Arc<Mutex<DaemonState>>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut last_acf_mtime = std::time::SystemTime::UNIX_EPOCH;
            let mut last_config_mtimes: HashMap<PathBuf, std::time::SystemTime> = HashMap::new();
            let mut last_applied_paths: HashMap<String, PathBuf> = HashMap::new();
            let mut is_initial_scan = true;
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));

            debug!("Steam and config.json watcher started");

            loop {
                interval.tick().await;

                let (config_targets, workshop_targets) = Self::get_watch_targets();

                // 1. Check for Steam Workshop updates (new subscriptions in Steam client)
                for acf in workshop_targets {
                    if let Ok(meta) = fs::metadata(&acf) {
                        if let Ok(mtime) = meta.modified() {
                            if mtime > last_acf_mtime && last_acf_mtime != std::time::SystemTime::UNIX_EPOCH {
                                info!("Detected Steam Workshop changes in {:?}", acf);
                                let mut s = state.lock().await;
                                s.rescan();
                            }
                            last_acf_mtime = mtime;
                        }
                    }
                }

                // 2. Check for Wallpaper Engine config.json changes (selected in official UI)
                for cfg in config_targets {
                    let mtime = match fs::metadata(&cfg).and_then(|m| m.modified()) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };

                    let is_initial = !last_config_mtimes.contains_key(&cfg);
                    let prev_mtime = last_config_mtimes.get(&cfg).copied().unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                    if !is_initial && mtime <= prev_mtime {
                        // File has not been modified since last check
                        continue;
                    }

                    last_config_mtimes.insert(cfg.clone(), mtime);

                    if let Ok(content) = fs::read_to_string(&cfg) {
                        let selections = Self::parse_config_json(&content);
                        for (mon, path) in selections {
                            let prev = last_applied_paths.get(&mon);
                            if prev != Some(&path) {
                                last_applied_paths.insert(mon.clone(), path.clone());

                                // On initial startup scan, do not overwrite if a wallpaper was already restored
                                if is_initial_scan {
                                    let s = state.lock().await;
                                    if !s.render_manager.get_active_wallpapers().is_empty() {
                                        continue;
                                    }
                                }

                                info!("Steam Wallpaper Engine selected wallpaper for {}: {:?}", mon, path);
                                if let Some(id) = Self::extract_wallpaper_id(&path) {
                                    let mut s = state.lock().await;
                                    // On startup, only apply if no wallpaper is currently running
                                    let should_apply = if is_initial {
                                        s.render_manager.get_active_wallpapers().is_empty()
                                    } else {
                                        true
                                    };

                                    if should_apply {
                                        let target_mon = Self::map_steam_monitor(&mon, &s.monitors);
                                        info!(
                                            "Applying Steam Wallpaper Engine selection '{}' for {} (target: {:?})",
                                            id, mon, target_mon
                                        );
                                        let _ = s.apply_by_query(&id, target_mon.as_deref()).await;
                                    }
                                }
                            }
                        }
                    }
                }
                is_initial_scan = false;
            }
        })
    }
}
