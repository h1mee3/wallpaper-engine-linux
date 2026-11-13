use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const DESKTOP_TEMPLATE: &str = include_str!("../assets/wengine-ui.desktop");
const APP_ICON_SVG: &str = include_str!("../assets/wengine.svg");
const RELEASES_API: &str =
    "https://api.github.com/repos/h1mee3/wallpaper-engine-linux/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    pub download_url: Option<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResult {
    pub updated: bool,
    pub version: String,
    pub installed_to: PathBuf,
    pub message: String,
}

fn parse_semver(v: &str) -> (u64, u64, u64) {
    let clean = v.trim().trim_start_matches('v');
    let mut parts = clean.split('.').map(|p| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

pub fn check_for_updates() -> Result<UpdateInfo> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: wengine-updater",
            RELEASES_API,
        ])
        .output()
        .context("Failed to execute curl to check GitHub releases")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("GitHub API request failed: {}", stderr.trim()));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse GitHub release JSON")?;

    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or(CURRENT_VERSION)
        .to_string();
    let release_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/h1mee3/wallpaper-engine-linux/releases")
        .to_string();
    let notes = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut download_url = None;
    if let Some(assets) = json.get("assets").and_then(|v| v.as_array()) {
        for asset in assets {
            let name = asset.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name == "wengine-linux-x86_64.tar.gz"
                || (name.starts_with("wengine-") && name.ends_with("-linux-x86_64.tar.gz"))
            {
                if let Some(url) = asset.get("browser_download_url").and_then(|v| v.as_str()) {
                    download_url = Some(url.to_string());
                    if name == "wengine-linux-x86_64.tar.gz" {
                        break;
                    }
                }
            }
        }
    }

    let update_available = parse_semver(&tag) > parse_semver(CURRENT_VERSION);

    Ok(UpdateInfo {
        current_version: format!("v{}", CURRENT_VERSION),
        latest_version: tag,
        update_available,
        release_url,
        download_url,
        notes,
    })
}

pub fn apply_update() -> Result<UpdateResult> {
    let info = check_for_updates()?;
    let url = info
        .download_url
        .ok_or_else(|| anyhow!("No Linux x86_64 release archive found in {}", info.latest_version))?;

    let tmp_dir = std::env::temp_dir().join(format!("wengine-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join("release.tar.gz");

    let dl = Command::new("curl")
        .args(["-fsSL", "-L", "-o"])
        .arg(&archive_path)
        .arg(&url)
        .status()
        .context("Failed to download release archive")?;

    if !dl.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(anyhow!("Failed to download release from {}", url));
    }

    let extract_dir = tmp_dir.join("extracted");
    std::fs::create_dir_all(&extract_dir)?;

    let tar_status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&extract_dir)
        .status()
        .context("Failed to extract release archive")?;

    if !tar_status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(anyhow!("Failed to extract downloaded tarball"));
    }

    let install_dir = dirs::home_dir()
        .map(|h| h.join(".local/bin"))
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin"));
    std::fs::create_dir_all(&install_dir)?;

    let binaries = [
        "wengine",
        "wengine-layer-player",
        "wengine-web-player",
        "wengine-ui",
    ];

    let mut installed_count = 0;
    for bin in &binaries {
        let src = extract_dir.join(bin);
        if src.exists() {
            let dst = install_dir.join(bin);
            // Remove existing file/symlink first so atomic replacement succeeds even if running
            let _ = std::fs::remove_file(&dst);
            std::fs::copy(&src, &dst)
                .with_context(|| format!("Failed to install {} to {:?}", bin, dst))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755));
            }
            installed_count += 1;
        }
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);

    if installed_count == 0 {
        return Err(anyhow!("Release archive contained no wengine binaries"));
    }

    install_desktop_entry(&install_dir);

    Ok(UpdateResult {
        updated: true,
        version: info.latest_version.clone(),
        installed_to: install_dir,
        message: format!(
            "Updated {} binaries to {} (restart daemon/UI to use new version)",
            installed_count, info.latest_version
        ),
    })
}

fn install_desktop_entry(install_dir: &std::path::Path) {
    let data_home = dirs::data_local_dir().or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
    let Some(data_home) = data_home else {
        return;
    };

    let exec_path = install_dir.join("wengine");
    let desktop_content = DESKTOP_TEMPLATE.replace(
        "Exec=wengine ui",
        &format!("Exec={} ui", exec_path.to_string_lossy()),
    );

    let apps_dir = data_home.join("applications");
    if std::fs::create_dir_all(&apps_dir).is_ok() {
        if std::fs::write(apps_dir.join("wengine.desktop"), desktop_content).is_ok() {
            let _ = Command::new("update-desktop-database").arg(&apps_dir).status();
        }
    }

    let icons_dir = data_home.join("icons/hicolor/scalable/apps");
    if std::fs::create_dir_all(&icons_dir).is_ok() {
        let _ = std::fs::write(icons_dir.join("wengine.svg"), APP_ICON_SVG);
    }
}
