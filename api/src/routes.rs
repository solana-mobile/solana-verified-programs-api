use crate::{
    db::Db,
    handlers::{read, system, verify, webhooks},
};
use axum::{
    http::{Method, Request},
    response::Response,
    routing::{get, post},
    Router,
};
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor, GovernorLayer,
};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::Span;

/// Wires every route, middleware, and rate limiter onto a fresh [`Router`].
pub fn build(db: Db) -> Router {
    let rate_limit_per_ip = |period_secs: u64, burst: u32| {
        let cfg = Box::leak(Box::new(
            GovernorConfigBuilder::default()
                .per_second(period_secs)
                .burst_size(burst)
                .use_headers()
                .key_extractor(SmartIpKeyExtractor)
                .finish()
                .unwrap(),
        ));
        GovernorLayer { config: cfg }
    };

    let cors = |m: Method| CorsLayer::new().allow_methods(m).allow_origin(Any);

    let trace = TraceLayer::new_for_http()
        .make_span_with(|r: &Request<_>| {
            tracing::info_span!("http", method = %r.method(), path = r.uri().path())
        })
        .on_response(|res: &Response, latency: std::time::Duration, _: &Span| {
            tracing::info!(latency = ?latency, status = res.status().as_u16(), "done");
        });

    Router::new()
        .route("/verify", post(verify::verify_async))
        .route("/verify-with-signer", post(verify::verify_with_signer))
        .route("/verify_sync", post(verify::verify_sync))
        .route_layer(rate_limit_per_ip(30, 1))
        .route("/unverify", post(webhooks::unverify))
        .route("/pda", post(webhooks::pda))
        .route_layer(rate_limit_per_ip(1, 100))
        .route("/status/:program_id", get(read::status))
        .route("/status-all/:program_id", get(read::status_all))
        .route("/resolve-hash/:hash", get(read::resolve_hash))
        .route("/job/:job_id", get(read::job))
        .route("/logs/:build_id", get(read::build_logs))
        .route("/verified-programs", get(read::verified_programs))
        .route(
            "/verified-programs/:page",
            get(read::verified_programs_paginated),
        )
        .route(
            "/verified-programs-status",
            get(read::verified_programs_status),
        )
        .route("/health/background-jobs", get(read::background_job_status))
        .route_layer(rate_limit_per_ip(1, 100))
        .route("/", get(system::landing))
        .route("/api", get(system::index))
        .route("/health", get(system::health))
        .layer(cors(Method::GET))
        .layer(CompressionLayer::new().zstd(true))
        .layer(trace)
        .with_state(db)
}
