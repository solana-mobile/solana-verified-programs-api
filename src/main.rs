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
mod types;

use config::CONFIG;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let db = db::Db::connect(&CONFIG.database_url)
        .await
        .expect("connect db");
    db.migrate().await.expect("apply migrations");

    sweep::spawn(db.clone());

    let app = routes::build(db);
    let addr = SocketAddr::from(([0, 0, 0, 0], CONFIG.port));
    tracing::info!("listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("serve");
}
