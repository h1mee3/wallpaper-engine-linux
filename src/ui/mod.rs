use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::discovery::Wallpaper;
use crate::ipc::{IpcClient, IpcRequest, IpcResponse, PauseMode};
use crate::renderer::ScalingMode;

const INDEX_HTML: &str = include_str!("index.html");

fn find_ui_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let p = parent.join("wengine-ui");
            if p.exists() {
                return p;
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/bin/wengine-ui");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("wengine-ui")
}

async fn ensure_daemon_running() {
    if IpcClient::send(IpcRequest::Status).await.is_ok() {
        return;
    }

    info!("wengine daemon is not running; starting it in background...");
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if IpcClient::send(IpcRequest::Status).await.is_ok() {
                return;
            }
        }
    }
    warn!("Could not connect to wengine daemon; UI will still start");
}

pub async fn run_ui(web_only: bool) -> Result<()> {
    ensure_daemon_running().await;

    let preview_cache: Arc<RwLock<HashMap<String, PathBuf>>> = Arc::new(RwLock::new(HashMap::new()));

    // Pre-populate preview cache from daemon list
    if let Ok(IpcResponse::Ok { data }) = IpcClient::send(IpcRequest::List).await {
        if let Ok(wps) = serde_json::from_value::<Vec<Wallpaper>>(data) {
            let mut cache = preview_cache.write().await;
            for w in wps {
                if let Some(p) = w.preview_path {
                    cache.insert(w.id, p);
                }
            }
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("Failed to bind local UI server")?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{}/", port);

    let cache_clone = preview_cache.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let cache = cache_clone.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_http(stream, cache).await {
                            error!("UI HTTP error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("UI accept error: {}", e);
                    break;
                }
            }
        }
    });

    if web_only {
        println!("🌌 wengine UI running at {}", url);
        let _ = std::process::Command::new("xdg-open")
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        tokio::signal::ctrl_c().await?;
        return Ok(());
    }

    let ui_bin = find_ui_binary();
    match tokio::process::Command::new(&ui_bin).arg(&url).status().await {
        Ok(status) => {
            if !status.success() {
                warn!("wengine-ui exited with status: {}", status);
            }
        }
        Err(e) => {
            warn!("Failed to launch wengine-ui ({}): {}. Falling back to browser...", ui_bin.display(), e);
            println!("🌌 wengine UI running at {}", url);
            let _ = std::process::Command::new("xdg-open")
                .arg(&url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            tokio::signal::ctrl_c().await?;
        }
    }

    Ok(())
}

async fn handle_http(
    mut stream: TcpStream,
    preview_cache: Arc<RwLock<HashMap<String, PathBuf>>>,
) -> Result<()> {
    let mut buf = vec![0u8; 16384];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    // Read any remaining body if Content-Length exceeds first read
    let header_end = buf[..n]
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(n);

    let header_str = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_str.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let raw_path = parts.next().unwrap_or("/");
    let path = raw_path.split('?').next().unwrap_or("/");

    let mut content_len: usize = 0;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_len = v.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut body_bytes = buf[header_end..n].to_vec();
    while body_bytes.len() < content_len {
        let mut extra = vec![0u8; content_len - body_bytes.len()];
        let r = stream.read(&mut extra).await?;
        if r == 0 {
            break;
        }
        body_bytes.extend_from_slice(&extra[..r]);
    }

    let body_json: Value = if !body_bytes.is_empty() {
        serde_json::from_slice(&body_bytes).unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    if method == "GET" && path == "/" {
        write_response(&mut stream, "200 OK", "text/html; charset=utf-8", INDEX_HTML.as_bytes(), false).await?;
        return Ok(());
    }

    if method == "GET" && path.starts_with("/api/preview/") {
        let encoded_id = &path["/api/preview/".len()..];
        let id = url_decode(encoded_id);
        let cache = preview_cache.read().await;
        if let Some(file_path) = cache.get(&id) {
            if let Ok(bytes) = tokio::fs::read(file_path).await {
                let mime = guess_mime(file_path);
                write_response(&mut stream, "200 OK", mime, &bytes, true).await?;
                return Ok(());
            }
        }
        write_response(&mut stream, "404 Not Found", "text/plain", b"Preview not found", false).await?;
        return Ok(());
    }

    let result = match (method, path) {
        ("GET", "/api/wallpapers") => match IpcClient::send(IpcRequest::List).await {
            Ok(IpcResponse::Ok { data }) => {
                if let Ok(wps) = serde_json::from_value::<Vec<Wallpaper>>(data.clone()) {
                    let mut cache = preview_cache.write().await;
                    for w in wps {
                        if let Some(p) = w.preview_path {
                            cache.insert(w.id, p);
                        }
                    }
                }
                Ok(data)
            }
            Ok(IpcResponse::Error { message }) => Err(message),
            Err(e) => Err(e.to_string()),
        },
        ("GET", "/api/status") => ipc_to_json(IpcRequest::Status).await,
        ("GET", "/api/update/check") => {
            match tokio::task::spawn_blocking(crate::updater::check_for_updates).await {
                Ok(Ok(info)) => serde_json::to_value(info).map_err(|e| e.to_string()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
        ("POST", "/api/update/apply") => {
            match tokio::task::spawn_blocking(crate::updater::apply_update).await {
                Ok(Ok(res)) => serde_json::to_value(res).map_err(|e| e.to_string()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
        ("POST", "/api/apply") => {
            let id = body_json.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let monitor = body_json.get("monitor").and_then(|v| v.as_str()).map(|s| s.to_string());
            ipc_to_json(IpcRequest::Apply { id, monitor }).await
        }
        ("POST", "/api/sync-steam") => ipc_to_json(IpcRequest::SyncSteam).await,
        ("POST", "/api/toggle") => ipc_to_json(IpcRequest::Toggle).await,
        ("POST", "/api/next") => ipc_to_json(IpcRequest::Next { monitor: None }).await,
        ("POST", "/api/prev") => ipc_to_json(IpcRequest::Prev { monitor: None }).await,
        ("POST", "/api/random") => ipc_to_json(IpcRequest::Random { monitor: None }).await,
        ("POST", "/api/rescan") => ipc_to_json(IpcRequest::Rescan).await,
        ("POST", "/api/volume") => {
            let value = body_json.get("value").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
            ipc_to_json(IpcRequest::Volume { value }).await
        }
        ("POST", "/api/mute") => {
            let value = body_json.get("value").and_then(|v| v.as_bool()).unwrap_or(false);
            ipc_to_json(IpcRequest::Mute { value }).await
        }
        ("POST", "/api/scaling") => {
            let mode_str = body_json.get("mode").and_then(|v| v.as_str()).unwrap_or("fill");
            let mode = match mode_str {
                "fit" => ScalingMode::Fit,
                "stretch" => ScalingMode::Stretch,
                _ => ScalingMode::Fill,
            };
            ipc_to_json(IpcRequest::Scaling { mode }).await
        }
        ("POST", "/api/pause-mode") => {
            let pm_str = body_json.get("mode").and_then(|v| v.as_str()).unwrap_or("fullscreen");
            let mode = match pm_str {
                "always" => PauseMode::Always,
                "power-save" | "covered" => PauseMode::PowerSave,
                _ => PauseMode::Fullscreen,
            };
            ipc_to_json(IpcRequest::SetPauseMode { mode }).await
        }
        _ => {
            write_response(&mut stream, "404 Not Found", "application/json", br#"{"error":"Not found"}"#, false).await?;
            return Ok(());
        }
    };

    match result {
        Ok(data) => {
            let json_bytes = serde_json::to_vec(&data)?;
            write_response(&mut stream, "200 OK", "application/json", &json_bytes, false).await?;
        }
        Err(msg) => {
            let err_bytes = serde_json::to_vec(&json!({ "error": msg }))?;
            write_response(&mut stream, "400 Bad Request", "application/json", &err_bytes, false).await?;
        }
    }

    Ok(())
}

async fn ipc_to_json(req: IpcRequest) -> std::result::Result<Value, String> {
    match IpcClient::send(req).await {
        Ok(IpcResponse::Ok { data }) => Ok(data),
        Ok(IpcResponse::Error { message }) => Err(message),
        Err(e) => Err(e.to_string()),
    }
}

async fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    cacheable: bool,
) -> Result<()> {
    let cache_hdr = if cacheable {
        "Cache-Control: public, max-age=3600\r\n"
    } else {
        "Cache-Control: no-store\r\n"
    };
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
        status,
        content_type,
        body.len(),
        cache_hdr
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

fn guess_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/jpeg",
    }
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
