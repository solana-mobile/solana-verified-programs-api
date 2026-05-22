use serde::Deserialize;

/// Configuration for the API server
#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    /// PostgreSQL database URL
    pub database_url: String,
    /// RPC URL Note: get_program_accounts call should be enabled on the RPC node
    pub rpc_url: String,
    /// List of RPC URLs for key rotation (comma-separated)
    /// If not provided, falls back to rpc_url
    pub rpc_urls: Option<String>,
    /// Auth secret
    pub auth_secret: String,
    /// Port to run the server on
    pub port: u16,
    /// Interval in seconds for the program_state sweep (default: 300 = 5 min)
    #[serde(default = "default_sweep_interval")]
    pub sweep_interval_seconds: u64,
    /// Maximum size of the Postgres connection pool (default: 50)
    #[serde(default = "default_db_max_connections")]
    pub db_max_connections: u32,
}

/// Default sweep interval: 5 minutes
fn default_sweep_interval() -> u64 {
    300
}

/// Default pool size: 50 connections
fn default_db_max_connections() -> u32 {
    50
}
