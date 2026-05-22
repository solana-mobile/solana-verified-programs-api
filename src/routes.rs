use crate::{
    db::Db,
    handlers::{
        async_verify, health, index, job_status, logs, pda_worker, resolve_hash, sync_verify,
        unverify, verification_status, verified_programs_list, verified_programs_status,
    },
};
use axum::http::Request;
use axum::{
    error_handling::HandleErrorLayer,
    http::{Method, StatusCode},
    response::Response,
    routing::{get, post},
    BoxError, Router,
};
use std::time::Duration;
use tower::{buffer::BufferLayer, limit::RateLimitLayer, ServiceBuilder};
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor, GovernorLayer,
};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::Span;

pub fn initialize_router(db: Db) -> Router {
    let error_handler = || {
        ServiceBuilder::new().layer(HandleErrorLayer::new(|err: BoxError| async move {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Unhandled error: {err}"),
            )
        }))
    };

    let global_rate_limit = |req_per_sec: u64| {
        ServiceBuilder::new()
            .layer(error_handler())
            .layer(BufferLayer::new(1024))
            .layer(RateLimitLayer::new(req_per_sec, Duration::from_secs(1)))
    };

    let rate_limit_per_ip = |timeout: u64, limit: u32| {
        let config = std::sync::Arc::new(
            GovernorConfigBuilder::default()
                .per_second(timeout)
                .burst_size(limit)
                .use_headers()
                .key_extractor(SmartIpKeyExtractor)
                .finish()
                .unwrap(),
        );

        // tower_governor 0.8 handles its own errors; no HandleErrorLayer needed.
        GovernorLayer::new(config)
    };

    let cors = |method: Method| {
        ServiceBuilder::new().layer(CorsLayer::new().allow_methods(method).allow_origin(Any))
    };

    // Per-request tracing emits at debug — set `RUST_LOG=verified_programs_api=debug`
    // to see every request. 4xx/5xx still bubble up via axum's error handling.
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|request: &Request<_>| {
            tracing::debug_span!(
                "http_request",
                method = %request.method(),
                path = request.uri().path(),
            )
        })
        .on_response(|response: &Response, latency: Duration, _span: &Span| {
            tracing::debug!(
                latency = ?latency,
                status = response.status().as_u16(),
                "finished processing request"
            );
        });

    // Define routes with their rate limits
    Router::new()
        // Verification routes (stricter rate limits)
        .route(
            "/verify",
            post(async_verify::process_async_verification),
        )
        .route(
            "/verify-with-signer",
            post(async_verify::process_async_verification_with_signer),
        )
        .route(
            "/verify_sync",
            post(sync_verify::process_sync_verification),
        )
        .layer(
            global_rate_limit(5)
                .layer(rate_limit_per_ip(30, 1))
                .layer(cors(Method::POST)),
        )
        .route("/unverify", post(unverify::handle_unverify))
        .layer(
            global_rate_limit(100)
                .layer(rate_limit_per_ip(1, 100))
                .layer(cors(Method::POST)),
        )
        .route(
            "/status-all/:address",
            get(verification_status::get_verification_status_all),
        )
        .route(
            "/status/:address",
            get(verification_status::get_verification_status),
        )
        .route("/resolve-hash/:hash", get(resolve_hash::resolve))
        .route("/job/:job_id", get(job_status::get_job_status))
        .route("/logs/:build_id", get(logs::get_build_logs))
        .route("/pda", post(pda_worker::handle_pda_updates_creations))
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
        .layer(
            global_rate_limit(10000)
                .layer(rate_limit_per_ip(1, 100))
                .layer(cors(Method::GET)),
        )
        // Base route
        .route("/", get(index::landing))
        .route("/api", get(index::endpoints))
        .route("/health", get(health::health_check))
        .route(
            "/health/background-jobs",
            get(health::background_job_status),
        )
        // Apply common middleware. Compression is at the outermost layer so
        // it doesn't need to re-derive body types through the rate-limit
        // stack (which broke under axum 0.7's stricter body bounds).
        .layer(CompressionLayer::new().zstd(true))
        .layer(trace_layer)
        .with_state(db)
}
