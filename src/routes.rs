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
            .route("/verify", post(async_verify::verify))
            .route(
                "/verify-with-signer",
                post(async_verify::verify_with_signer)
            )
            .route("/verify_sync", post(sync_verify::verify_sync)),
        30,
        1
    );
    let webhook_group: Router<Db> = maybe_rl!(
        Router::new()
            .route("/unverify", post(unverify::unverify))
            .route("/pda", post(pda_worker::pda)),
        1,
        100
    );
    let read_group: Router<Db> = maybe_rl!(
        Router::new()
            .route("/status/:address", get(verification_status::status))
            .route("/status-all/:address", get(verification_status::status_all))
            .route("/resolve-hash/:hash", get(resolve_hash::resolve))
            .route("/job/:job_id", get(job_status::status))
            .route("/logs/:build_id", get(logs::fetch))
            .route("/verified-programs", get(verified_programs_list::list))
            .route(
                "/verified-programs/:page",
                get(verified_programs_list::paginated),
            )
            .route(
                "/verified-programs-status",
                get(verified_programs_status::all),
            )
            .route("/health/background-jobs", get(health::background_jobs)),
        1,
        100
    );

    Router::new()
        .merge(verify_group)
        .merge(webhook_group)
        .merge(read_group)
        .route("/", get(index::landing))
        .route("/api", get(index::endpoints))
        .route("/health", get(health::health))
        .layer(cors(Method::GET))
        .layer(CompressionLayer::new().zstd(true))
        .layer(trace)
        .with_state(db)
}
