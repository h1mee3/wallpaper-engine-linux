use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/c/layer_player.c");
    println!("cargo:rerun-if-changed=src/c/web_player.c");
    println!("cargo:rerun-if-changed=src/c/ui_window.c");
    println!("cargo:rerun-if-changed=src/c/wlr-layer-shell.c");
    println!("cargo:rerun-if-changed=src/c/wlr-layer-shell.h");
    println!("cargo:rerun-if-changed=src/c/xdg-shell.c");
    println!("cargo:rerun-if-changed=src/c/xdg-shell.h");

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
            if let Ok(home) = std::env::var("HOME") {
                let local_bin = std::path::PathBuf::from(home).join(".local/bin/wengine-layer-player");
                let same_file = match (dest.canonicalize(), local_bin.canonicalize()) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                };
                if !same_file {
                    let _ = std::fs::remove_file(&local_bin);
                    let _ = std::fs::copy(&dest, &local_bin);
                }
            }
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
            if let Ok(home) = std::env::var("HOME") {
                let local_bin = std::path::PathBuf::from(home).join(".local/bin/wengine-web-player");
                let same_file = match (dest_web.canonicalize(), local_bin.canonicalize()) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                };
                if !same_file {
                    let _ = std::fs::remove_file(&local_bin);
                    let _ = std::fs::copy(&dest_web, &local_bin);
                }
            }
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
            if let Ok(home) = std::env::var("HOME") {
                let local_bin = std::path::PathBuf::from(home).join(".local/bin/wengine-ui");
                let same_file = match (dest_ui.canonicalize(), local_bin.canonicalize()) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                };
                if !same_file {
                    let _ = std::fs::remove_file(&local_bin);
                    let _ = std::fs::copy(&dest_ui, &local_bin);
                }
            }
        }
        _ => {
            println!("cargo:warning=Failed to compile wengine-ui helper");
        }
    }
}
