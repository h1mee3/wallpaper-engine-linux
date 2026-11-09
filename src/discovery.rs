use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};


use crate::vdf::{extract_library_paths, parse_vdf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperType {
    Video,
    Scene,
    Web,
    Application,
    #[serde(untagged)]
    Other(String),
}

impl Default for WallpaperType {
    fn default() -> Self {
        WallpaperType::Other("unknown".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wallpaper {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub wallpaper_type: WallpaperType,
    pub file: String,
    pub full_file_path: PathBuf,
    pub preview_path: Option<PathBuf>,
    pub folder_path: PathBuf,
    pub tags: Vec<String>,
    pub content_rating: Option<String>,
    pub properties: Option<HashMap<String, serde_json::Value>>,
}

/// Raw representation of Wallpaper Engine `project.json`
#[derive(Debug, Deserialize)]
struct RawProjectJson {
    title: Option<String>,
    description: Option<String>,
    #[serde(rename = "type")]
    wallpaper_type: Option<String>,
    file: Option<String>,
    preview: Option<String>,
    tags: Option<Vec<String>>,
    contentrating: Option<String>,
    general: Option<RawGeneral>,
}

#[derive(Debug, Deserialize)]
struct RawGeneral {
    properties: Option<HashMap<String, serde_json::Value>>,
}

pub struct Discovery;

impl Discovery {
    /// Detect all steam library folders on the system
    pub fn find_steam_libraries() -> Vec<PathBuf> {
        let mut libraries = Vec::new();

        let mut candidate_roots = Vec::new();
        if let Some(home) = dirs::home_dir() {
            candidate_roots.push(home.join(".local/share/Steam"));
            candidate_roots.push(home.join(".steam/steam"));
            candidate_roots.push(home.join(".steam/root"));
        }
        candidate_roots.push(PathBuf::from("/hdd/SteamLibrary"));
        candidate_roots.push(PathBuf::from("/run/media"));

        for root in &candidate_roots {
            let vdf_path = root.join("steamapps/libraryfolders.vdf");
            if vdf_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&vdf_path) {
                    if let Ok(parsed) = parse_vdf(&content) {
                        for path in extract_library_paths(&parsed) {
                            if !libraries.contains(&path) {
                                libraries.push(path);
                            }
                        }
                    }
                }
            } else if root.join("steamapps").exists() && !libraries.contains(root) {
                libraries.push(root.clone());
            }
        }

        // Also check if any mounted filesystems have SteamLibrary
        if let Ok(entries) = std::fs::read_dir("/run/media") {
            for entry in entries.flatten() {
                if let Ok(subentries) = std::fs::read_dir(entry.path()) {
                    for sub in subentries.flatten() {
                        let steam_lib = sub.path().join("SteamLibrary");
                        if steam_lib.exists() && !libraries.contains(&steam_lib) {
                            libraries.push(steam_lib);
                        }
                    }
                }
            }
        }

        libraries
    }

    /// Discover all wallpapers across Steam libraries and custom paths
    pub fn discover_all(custom_paths: &[PathBuf]) -> Vec<Wallpaper> {
        let mut wallpapers = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        let steam_libs = Self::find_steam_libraries();
        debug!("Discovered {} Steam libraries", steam_libs.len());

        let mut search_directories = Vec::new();

        for lib in &steam_libs {
            // Default projects (Neon Sunset, Beach, Techno, etc.)
            search_directories.push(lib.join("steamapps/common/wallpaper_engine/projects/defaultprojects"));
            search_directories.push(lib.join("steamapps/common/wallpaper_engine/projects/templates"));

            // Subscribed / downloaded workshop wallpapers
            search_directories.push(lib.join("steamapps/workshop/content/431960"));
            search_directories.push(lib.join("steamapps/workshop/downloads/431960"));
        }

        // Custom user paths
        if let Some(data_dir) = dirs::data_dir() {
            search_directories.push(data_dir.join("wengine/wallpapers"));
            search_directories.push(data_dir.join("wallpaper-engine/wallpapers"));
        }
        for p in custom_paths {
            search_directories.push(p.clone());
        }

        for dir in search_directories {
            if !dir.exists() {
                continue;
            }

            debug!("Scanning for wallpapers in {:?}", dir);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Some(wp) = Self::load_wallpaper_from_dir(&path) {
                            if seen_ids.insert(wp.id.clone()) {
                                wallpapers.push(wp);
                            }
                        }
                    }
                }
            }
        }

        wallpapers.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        info!("Discovered {} total wallpapers", wallpapers.len());
        wallpapers
    }

    /// Load a single wallpaper from a folder containing `project.json`
    pub fn load_wallpaper_from_dir(dir: &Path) -> Option<Wallpaper> {
        let project_json_path = dir.join("project.json");
        if !project_json_path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&project_json_path).ok()?;
        let raw: RawProjectJson = serde_json::from_str(&content).ok()?;

        let folder_name = dir.file_name()?.to_string_lossy().to_string();
        let title = raw.title.unwrap_or_else(|| folder_name.clone());

        let raw_type = raw.wallpaper_type.unwrap_or_default().to_lowercase();
        let wallpaper_type = match raw_type.as_str() {
            "video" => WallpaperType::Video,
            "scene" => WallpaperType::Scene,
            "web" => WallpaperType::Web,
            "application" => WallpaperType::Application,
            other => WallpaperType::Other(other.to_string()),
        };

        let file_name = raw.file.unwrap_or_else(|| {
            // Default heuristic if not explicitly set
            if dir.join("scene.pkg").exists() {
                "scene.pkg".to_string()
            } else if dir.join("scene.json").exists() {
                "scene.json".to_string()
            } else if dir.join("index.html").exists() {
                "index.html".to_string()
            } else {
                String::new()
            }
        });

        // Resolve the actual file path
        let mut full_file_path = dir.join(&file_name);
        // If file points to scene.json but scene.pkg exists, prioritize scene.pkg
        if (!full_file_path.exists() || file_name == "scene.json") && dir.join("scene.pkg").exists() {
            full_file_path = dir.join("scene.pkg");
        }

        // Resolve preview image
        let preview_path = raw.preview.map(|p| dir.join(p)).filter(|p| p.exists()).or_else(|| {
            // Fallback search for preview.jpg, preview.png, preview.gif
            for ext in &["jpg", "png", "gif", "jpeg", "webp"] {
                let candidate = dir.join(format!("preview.{}", ext));
                if candidate.exists() {
                    return Some(candidate);
                }
            }
            None
        });

        Some(Wallpaper {
            id: folder_name,
            title,
            description: raw.description,
            wallpaper_type,
            file: file_name,
            full_file_path,
            preview_path,
            folder_path: dir.to_path_buf(),
            tags: raw.tags.unwrap_or_default(),
            content_rating: raw.contentrating,
            properties: raw.general.and_then(|g| g.properties),
        })
    }
}
