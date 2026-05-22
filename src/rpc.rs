//! Round-robin RPC client pool that rotates on rate-limit-shaped errors.

use crate::{config::CONFIG, error::ApiError, error::Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, warn};

/// Round-robin pool of Solana RPC clients.
pub struct RpcManager {
    urls: Vec<String>,
    cursor: RwLock<usize>,
}

impl RpcManager {
    /// Builds a manager from `RPC_URLS` (comma-separated), falling back to
    /// a single-element list containing `RPC_URL`.
    pub fn new() -> Self {
        let urls: Vec<String> = match &CONFIG.rpc_urls {
            Some(joined) => joined
                .split(',')
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
                .collect(),
            None => vec![],
        };
        let urls = if urls.is_empty() {
            vec![CONFIG.rpc_url.clone()]
        } else {
            urls
        };
        Self {
            urls,
            cursor: RwLock::new(0),
        }
    }

    /// A client for the currently-selected URL.
    pub async fn client(&self) -> Arc<RpcClient> {
        let i = *self.cursor.read().await;
        Arc::new(RpcClient::new(self.urls[i].clone()))
    }

    async fn rotate(&self) -> Arc<RpcClient> {
        let mut i = self.cursor.write().await;
        *i = (*i + 1) % self.urls.len();
        warn!("Rotated RPC to index {}", *i);
        Arc::new(RpcClient::new(self.urls[*i].clone()))
    }

    /// Retries `op` against the next URL on rate-limit-shaped errors only.
    /// `op` may run more than once, so it must be idempotent.
    pub async fn run<F, Fut, T>(&self, op: F) -> Result<T>
    where
        F: Fn(Arc<RpcClient>) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<T>> + Send,
    {
        let max = self.urls.len();
        let mut last: Option<ApiError> = None;
        for attempt in 0..max {
            let c = if attempt == 0 {
                self.client().await
            } else {
                self.rotate().await
            };
            match op(c).await {
                Ok(v) => return Ok(v),
                Err(e) if is_rate_limited(&e) => {
                    warn!("rate-limited (attempt {}/{}): {}", attempt + 1, max, e);
                    last = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        error!("all {} RPC URLs exhausted", max);
        Err(last.unwrap_or_else(|| ApiError::Rpc("all rpcs exhausted".into())))
    }
}

fn is_rate_limited(err: &ApiError) -> bool {
    let s = err.to_string().to_lowercase();
    s.contains("time limit")
        || s.contains("timeout")
        || s.contains("rate limit")
        || s.contains("too many requests")
        || s.contains("429")
}

static RPC: once_cell::sync::Lazy<RpcManager> = once_cell::sync::Lazy::new(RpcManager::new);

/// The process-wide [`RpcManager`].
pub fn rpc() -> &'static RpcManager {
    &RPC
}
