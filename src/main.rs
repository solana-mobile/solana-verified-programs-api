use config::Config;
use std::net::SocketAddr;

mod build;
mod config;
mod db;
mod error;
mod handlers;
mod logs;
mod onchain;
mod response;
mod routes;
mod rpc;
mod sweep;
mod validation;

/// Result type for API
pub type Result<T> = std::result::Result<T, error::ApiError>;

/// Static configuration instance for the API
static CONFIG: once_cell::sync::Lazy<Config> = once_cell::sync::Lazy::new(|| {
    dotenvy::dotenv().ok();
    envy::from_env::<Config>().expect("Failed to load configuration")
});

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Initialize database connection
    let db = db::Db::connect(&CONFIG.database_url)
        .await
        .expect("Failed to connect to database");
    db.migrate().await.expect("Failed to apply migrations");

    // Start background jobs
    sweep::spawn(db.clone());

    // Setup API router and start server
    let app = routes::initialize_router(db);
    let addr = SocketAddr::from(([0, 0, 0, 0], CONFIG.port));
    tracing::info!("Server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
