use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub rpc_url: String,
    pub rpc_urls: Option<String>,
    pub auth_secret: String,
    pub port: u16,
    #[serde(default = "default_sweep_interval")]
    pub sweep_interval_seconds: u64,
}

fn default_sweep_interval() -> u64 {
    3600
}

pub static CONFIG: once_cell::sync::Lazy<Config> = once_cell::sync::Lazy::new(|| {
    dotenv::dotenv().ok();
    envy::from_env::<Config>().expect("Failed to load configuration from env")
});
