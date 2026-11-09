use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::renderer::ScalingMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum IpcRequest {
    List,
    Apply {
        id: String,
        monitor: Option<String>,
    },
    Status,
    Pause,
    Resume,
    Toggle,
    Volume {
        value: u32,
    },
    Mute {
        value: bool,
    },
    Scaling {
        mode: ScalingMode,
    },
    Next {
        monitor: Option<String>,
    },
    Prev {
        monitor: Option<String>,
    },
    Random {
        monitor: Option<String>,
    },
    Rescan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IpcResponse {
    Ok { data: serde_json::Value },
    Error { message: String },
}

pub fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("wengine.sock")
    } else {
        PathBuf::from("/tmp/wengine.sock")
    }
}

pub struct IpcClient;

impl IpcClient {
    /// Send an IPC request to the running wengine daemon and return response
    pub async fn send(req: IpcRequest) -> Result<IpcResponse> {
        let socket_path = get_socket_path();
        let mut stream = UnixStream::connect(&socket_path)
            .await
            .with_context(|| format!("Failed to connect to wengine daemon at {:?}", socket_path))?;

        let mut payload = serde_json::to_vec(&req)?;
        payload.push(b'\n');
        stream.write_all(&payload).await?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let resp: IpcResponse = serde_json::from_str(&line)?;
        Ok(resp)
    }
}
