use crate::{
    config::CONFIG,
    db::Db,
    handlers::{
        async_verify, health, index, job_status, logs, pda_worker, resolve_hash, sync_verify,
        unverify, verification_status, verified_programs_list, verified_programs_status,
    },
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

    // Per-request tracing emits at debug — set `RUST_LOG=verified_programs_api=debug`
    // to see every request. 4xx/5xx still bubble up via axum's error handling.
    let trace = TraceLayer::new_for_http()
        .make_span_with(|r: &Request<_>| {
            tracing::debug_span!("http", method = %r.method(), path = r.uri().path())
        })
        .on_response(|res: &Response, latency: std::time::Duration, _: &Span| {
            tracing::debug!(latency = ?latency, status = res.status().as_u16(), "done");
        });

    let verify_group: Router<Db> = maybe_rl!(
        Router::new()
            .route("/verify", post(async_verify::process_async_verification))
            .route(
                "/verify-with-signer",
                post(async_verify::process_async_verification_with_signer),
            )
            .route("/verify_sync", post(sync_verify::process_sync_verification)),
        30,
        1
    );
    let webhook_group: Router<Db> = maybe_rl!(
        Router::new()
            .route("/unverify", post(unverify::handle_unverify))
            .route("/pda", post(pda_worker::handle_pda_updates_creations)),
        1,
        100
    );
    let read_group: Router<Db> = maybe_rl!(
        Router::new()
            .route(
                "/status/:address",
                get(verification_status::get_verification_status),
            )
            .route(
                "/status-all/:address",
                get(verification_status::get_verification_status_all),
            )
            .route(
                "/resolve-hash/:hash",
                get(resolve_hash::get_builds_for_hash),
            )
            .route("/job/:job_id", get(job_status::get_job_status))
            .route("/logs/:build_id", get(logs::get_build_logs))
            .route(
                "/verified-programs",
                get(verified_programs_list::get_verified_programs_list),
            )
            .route(
                "/verified-programs/:page",
                get(verified_programs_list::get_verified_programs_list_paginated),
            )
            .route(
                "/verified-programs-status",
                get(verified_programs_status::get_verified_programs_status),
            )
            .route(
                "/health/background-jobs",
                get(health::background_job_status),
            ),
        1,
        100
    );

    Router::new()
        .merge(verify_group)
        .merge(webhook_group)
        .merge(read_group)
        .route("/", get(index::landing_page))
        .route("/api", get(index::index))
        .route("/health", get(health::health_check))
        .layer(cors(Method::GET))
        .layer(CompressionLayer::new().zstd(true))
        .layer(trace)
        .with_state(db)
}
