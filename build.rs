use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/c/layer_player.c");
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
            // Also install to ~/.local/bin if directory exists
            if let Ok(home) = std::env::var("HOME") {
                let local_bin = std::path::PathBuf::from(home).join(".local/bin/wengine-layer-player");
                let _ = std::fs::copy(&dest, local_bin);
            }
        }
        _ => {
            println!("cargo:warning=Failed to compile wengine-layer-player helper");
        }
    }
}
