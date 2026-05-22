use serde::Deserialize;

/// Field names map 1:1 to upper-cased env vars.
#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub rpc_url: String,
    /// Comma-separated fallback RPCs for [`crate::rpc::RpcManager`] rotation.
    /// Falls back to a single-element list containing `rpc_url` when unset.
    pub rpc_urls: Option<String>,
    /// Shared secret webhook callers must pass in `Authorization`.
    pub auth_secret: String,
    pub port: u16,
    #[serde(default = "default_sweep_interval")]
    pub sweep_interval_seconds: u64,
    /// Skip the per-IP `tower_governor` layers — useful for local
    /// benchmarking, never in production.
    #[serde(default)]
    pub disable_rate_limit: bool,
}

fn default_sweep_interval() -> u64 {
    300
}

pub static CONFIG: once_cell::sync::Lazy<Config> = once_cell::sync::Lazy::new(|| {
    dotenv::dotenv().ok();
    envy::from_env::<Config>().expect("Failed to load configuration from env")
});
