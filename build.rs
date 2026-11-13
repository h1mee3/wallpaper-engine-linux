use std::path::{Path, PathBuf};
use std::process::Command;

fn install_to_local_bin(src: &Path, name: &str) {
    if let Ok(home) = std::env::var("HOME") {
        let local_bin_dir = PathBuf::from(home).join(".local/bin");
        let _ = std::fs::create_dir_all(&local_bin_dir);
        let local_bin = local_bin_dir.join(name);
        let same_file = match (src.canonicalize(), local_bin.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same_file {
            let _ = std::fs::remove_file(&local_bin);
            let _ = std::fs::copy(src, &local_bin);
        }
    }
}

fn resolve_wengine_exec_path(home: &str) -> PathBuf {
    let local_bin = PathBuf::from(home).join(".local/bin/wengine");
    if local_bin.exists() {
        return local_bin;
    }
    let cargo_bin = PathBuf::from(home).join(".cargo/bin/wengine");
    if cargo_bin.exists() {
        return cargo_bin;
    }
    local_bin
}

fn install_desktop_entry(target_dir: &Path) {
    let Ok(template) = std::fs::read_to_string("assets/wengine-ui.desktop") else {
        println!("cargo:warning=Missing assets/wengine-ui.desktop; skipping desktop entry install");
        return;
    };

    let home = std::env::var("HOME").ok();
    let exec_bin = home
        .as_deref()
        .map(|h| resolve_wengine_exec_path(h).to_string_lossy().into_owned())
        .unwrap_or_else(|| "wengine".to_string());

    let desktop_content = template.replace("Exec=wengine ui", &format!("Exec={} ui", exec_bin));
    let _ = std::fs::write(target_dir.join("wengine.desktop"), &desktop_content);

    let Some(home) = home else {
        return;
    };

    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&home).join(".local/share"));

    let apps_dir = data_home.join("applications");
    if std::fs::create_dir_all(&apps_dir).is_ok() {
        let desktop_dst = apps_dir.join("wengine.desktop");
        if std::fs::write(&desktop_dst, &desktop_content).is_ok() {
            println!("cargo:warning=Installed desktop entry to {}", desktop_dst.display());
            let _ = Command::new("update-desktop-database").arg(&apps_dir).status();
        }
    }

    let icons_dir = data_home.join("icons/hicolor/scalable/apps");
    if std::fs::create_dir_all(&icons_dir).is_ok() {
        let _ = std::fs::copy("assets/wengine.svg", icons_dir.join("wengine.svg"));
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src/c/layer_player.c");
    println!("cargo:rerun-if-changed=src/c/web_player.c");
    println!("cargo:rerun-if-changed=src/c/ui_window.c");
    println!("cargo:rerun-if-changed=src/c/wlr-layer-shell.c");
    println!("cargo:rerun-if-changed=src/c/wlr-layer-shell.h");
    println!("cargo:rerun-if-changed=src/c/xdg-shell.c");
    println!("cargo:rerun-if-changed=src/c/xdg-shell.h");
    println!("cargo:rerun-if-changed=assets/wengine-ui.desktop");
    println!("cargo:rerun-if-changed=assets/wengine.svg");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let target_dir = std::path::Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .unwrap();

    let dest = target_dir.join("wengine-layer-player");

    let status = Command::new("gcc")
        .args([
            "-O3",
            "-Wall",
            "-Wextra",
            "src/c/layer_player.c",
            "src/c/wlr-layer-shell.c",
            "src/c/xdg-shell.c",
            "-o",
            dest.to_str().unwrap(),
            "-lwayland-client",
            "-lwayland-egl",
            "-lEGL",
            "-lGL",
            "-lmpv",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=Successfully built wengine-layer-player");
            install_to_local_bin(&dest, "wengine-layer-player");
        }
        _ => {
            println!("cargo:warning=Failed to compile wengine-layer-player helper");
        }
    }

    // Build wengine-web-player using WebKitGTK and GtkLayerShell (super lightweight, zero Electron)
    let dest_web = target_dir.join("wengine-web-player");
    let mut web_gcc = Command::new("gcc");
    web_gcc.args([
        "-O3",
        "-Wall",
        "-Wextra",
        "src/c/web_player.c",
        "-o",
        dest_web.to_str().unwrap(),
    ]);

    if let Ok(pkg) = Command::new("pkg-config")
        .args(["--cflags", "--libs", "gtk-layer-shell-0", "webkit2gtk-4.1"])
        .output()
    {
        if pkg.status.success() {
            let flags_str = String::from_utf8_lossy(&pkg.stdout);
            for flag in flags_str.split_whitespace() {
                web_gcc.arg(flag);
            }
        }
    }

    let status_web = web_gcc.status();
    match status_web {
        Ok(s) if s.success() => {
            println!("cargo:warning=Successfully built wengine-web-player");
            install_to_local_bin(&dest_web, "wengine-web-player");
        }
        _ => {
            println!("cargo:warning=Failed to compile wengine-web-player helper");
        }
    }

    // Build wengine-ui using GTK3 and WebKitGTK
    let dest_ui = target_dir.join("wengine-ui");
    let mut ui_gcc = Command::new("gcc");
    ui_gcc.args([
        "-O3",
        "-Wall",
        "-Wextra",
        "src/c/ui_window.c",
        "-o",
        dest_ui.to_str().unwrap(),
    ]);

    if let Ok(pkg) = Command::new("pkg-config")
        .args(["--cflags", "--libs", "gtk+-3.0", "webkit2gtk-4.1"])
        .output()
    {
        if pkg.status.success() {
            let flags_str = String::from_utf8_lossy(&pkg.stdout);
            for flag in flags_str.split_whitespace() {
                ui_gcc.arg(flag);
            }
        }
    }

    let status_ui = ui_gcc.status();
    match status_ui {
        Ok(s) if s.success() => {
            println!("cargo:warning=Successfully built wengine-ui");
            install_to_local_bin(&dest_ui, "wengine-ui");
        }
        _ => {
            println!("cargo:warning=Failed to compile wengine-ui helper");
        }
    }

    install_desktop_entry(target_dir);
}
