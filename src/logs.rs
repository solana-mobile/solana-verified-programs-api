//! Build logs written to `/logs` (mounted from the host). The configured RPC
//! URL is scrubbed before writing so private RPC keys don't land on disk.

use crate::{config::CONFIG, error::Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::error;

const LOGS_DIR: &str = "/logs";
const MAINNET_RPC: &str = "https://api.mainnet-beta.solana.com";

/// Persists `stderr` and `stdout` for a build under `file_id`.
pub async fn write(file_id: &str, stderr: &str, stdout: &str) -> Result<()> {
    let dir = Path::new(LOGS_DIR);
    if !dir.exists() {
        return Err(crate::error::ApiError::Internal(format!(
            "logs directory missing: {LOGS_DIR}"
        )));
    }
    let stderr = stderr.replace(&CONFIG.rpc_url, MAINNET_RPC);
    let stdout = stdout.replace(&CONFIG.rpc_url, MAINNET_RPC);
    write_file(&log_path(file_id, "err"), &stderr).await?;
    write_file(&log_path(file_id, "out"), &stdout).await?;
    Ok(())
}

/// Reads back the log pair for a build as a JSON object. Returns the stub
/// `{ error }` shape when both files are empty.
pub async fn read(file_id: &str) -> Value {
    let stderr = fs::read_to_string(log_path(file_id, "err"))
        .await
        .unwrap_or_else(|e| {
            error!("read stderr log: {}", e);
            String::new()
        });
    let stdout = fs::read_to_string(log_path(file_id, "out"))
        .await
        .unwrap_or_else(|e| {
            error!("read stdout log: {}", e);
            String::new()
        });
    if stderr.is_empty() && stdout.is_empty() {
        return json!({ "error": "We could not find the logs for this program" });
    }
    json!({ "std_err": stderr, "std_out": stdout })
}

fn log_path(file_id: &str, kind: &str) -> PathBuf {
    Path::new(LOGS_DIR).join(format!("{file_id}_{kind}.log"))
}

async fn write_file(path: &Path, content: &str) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await?;
    f.write_all(content.as_bytes()).await?;
    Ok(())
}
