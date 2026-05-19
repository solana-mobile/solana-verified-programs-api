use serde::Deserialize;

/// Configuration for the API server
#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    /// PostgreSQL database URL
    pub database_url: String,
    /// Redis URL
    pub redis_url: String,
    /// RPC URL. `get_program_accounts` must be enabled on the node.
    pub rpc_url: String,
    /// Comma-separated list of RPC URLs for key rotation. Falls back to `rpc_url`.
    pub rpc_urls: Option<String>,
    /// Port to run the server on
    pub port: u16,
}
