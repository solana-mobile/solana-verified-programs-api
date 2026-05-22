use crate::{
    config::CONFIG,
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

macro_rules! maybe_rl {
    ($router:expr, $period:expr, $burst:expr) => {{
        let r = $router;
        if CONFIG.disable_rate_limit {
            r
        } else {
            let cfg = Box::leak(Box::new(
                GovernorConfigBuilder::default()
                    .per_second($period)
                    .burst_size($burst)
                    .use_headers()
                    .key_extractor(SmartIpKeyExtractor)
                    .finish()
                    .unwrap(),
            ));
            r.route_layer(GovernorLayer { config: cfg })
        }
    }};
}

/// Wires every route, middleware, and rate limiter onto a fresh [`Router`].
pub fn build(db: Db) -> Router {
    let cors = |m: Method| CorsLayer::new().allow_methods(m).allow_origin(Any);

    let trace = TraceLayer::new_for_http()
        .make_span_with(|r: &Request<_>| {
            tracing::info_span!("http", method = %r.method(), path = r.uri().path())
        })
        .on_response(|res: &Response, latency: std::time::Duration, _: &Span| {
            tracing::info!(latency = ?latency, status = res.status().as_u16(), "done");
        });

    let verify_group: Router<Db> = maybe_rl!(
        Router::new()
            .route("/verify", post(verify::verify_async))
            .route("/verify-with-signer", post(verify::verify_with_signer))
            .route("/verify_sync", post(verify::verify_sync)),
        30,
        1
    );
    let webhook_group: Router<Db> = maybe_rl!(
        Router::new()
            .route("/unverify", post(webhooks::unverify))
            .route("/pda", post(webhooks::pda)),
        1,
        100
    );
    let read_group: Router<Db> = maybe_rl!(
        Router::new()
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
            .route("/health/background-jobs", get(read::background_job_status)),
        1,
        100
    );

    Router::new()
        .merge(verify_group)
        .merge(webhook_group)
        .merge(read_group)
        .route("/", get(system::landing))
        .route("/api", get(system::index))
        .route("/health", get(system::health))
        .layer(cors(Method::GET))
        .layer(CompressionLayer::new().zstd(true))
        .layer(trace)
        .with_state(db)
}
