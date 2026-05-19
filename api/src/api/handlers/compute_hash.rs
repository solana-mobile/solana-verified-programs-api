use super::verify_helpers::{
    create_and_insert_build, validate_program_id, validate_repository_url, validate_webhook_url,
};
use crate::db::models::{
    ApiResponse, ComputeHashParams, ComputeHashResponse, ErrorResponse, Status,
};
use crate::db::DbClient;
use crate::services::verification::{notify_webhook, process_verification_request};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use tracing::{error, info};

/// Handler for `POST /compute-hash`.
///
/// Takes a pure build config (no `program_id` is conceptually required) and
/// returns the deterministic executable hash it produces. If the
/// `(repository, commit, build_args)` triple has been verified before, the
/// cached hash is returned immediately. Otherwise a build is kicked off
/// asynchronously; clients poll `GET /job/:request_id` for completion, and
/// can then `GET /resolve-hash/:hash` to confirm the result.
///
/// Today's `solana-verify verify-from-repo` driver still requires a program
/// id to run, so if no cache entry exists, `program_id` must be supplied in
/// the request body. The directory entry it produces is content-addressed
/// and is not bound to this `program_id`.
pub(crate) async fn process_compute_hash(
    State(db): State<DbClient>,
    Json(payload): Json<ComputeHashParams>,
) -> (StatusCode, Json<ApiResponse>) {
    if let Err(resp) = validate_repository_url(&payload.repository) {
        return resp;
    }
    if let Err(resp) = validate_webhook_url(&payload.webhook_url) {
        return resp;
    }

    info!(
        "Compute-hash: looking up cached build for repository={} commit={:?} lib={:?}",
        payload.repository, payload.commit_hash, payload.lib_name
    );

    // Cache lookup: same `(repo, commit, build_args)` will collide on `executable_hash`.
    match db
        .find_verified_hash_by_config(
            &payload.repository,
            payload.commit_hash.as_deref(),
            payload.lib_name.as_deref(),
            payload.base_image.as_deref(),
            payload.mount_path.as_deref(),
            payload.cargo_args.as_deref(),
            payload.bpf_flag.unwrap_or(false),
            payload.arch.as_deref(),
        )
        .await
    {
        Ok(Some(cached)) => {
            info!(
                "Compute-hash: cache hit, executable_hash={}",
                cached.executable_hash
            );
            let body = ComputeHashResponse {
                status: "cached".to_string(),
                executable_hash: Some(cached.executable_hash),
                request_id: None,
                message: "Build already verified, returning cached hash".to_string(),
            };
            return (StatusCode::OK, Json(body.into()));
        }
        Ok(None) => {}
        Err(e) => {
            error!("Compute-hash: cache lookup failed: {:?}", e);
            // Fall through to attempt a build.
        }
    }

    // Cache miss: kick off a build. The current driver needs a program id.
    let Some(program_id) = payload.program_id.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                ErrorResponse {
                    status: Status::Error,
                    error: "No cached build for this configuration. Building from scratch \
                            currently requires a `program_id` in the request body."
                        .to_string(),
                }
                .into(),
            ),
        );
    };
    if let Err(resp) = validate_program_id(&program_id) {
        return resp;
    }

    let webhook_url = payload.webhook_url.clone();
    let build_params = payload.into_build_params(program_id);

    // Record the build row so /job/:request_id works.
    let request_id = match create_and_insert_build(&db, &build_params, "compute-hash").await {
        Ok(uuid) => uuid,
        Err(error_response) => return error_response,
    };

    let db_clone = db.clone();
    let request_id_clone = request_id.clone();
    tokio::spawn(async move {
        let result =
            process_verification_request(build_params, &request_id_clone, &db_clone).await;
        if let Err(e) = &result {
            error!("Compute-hash build failed: {:?}", e);
        }
        if let Some(url) = webhook_url {
            notify_webhook(url, result, request_id_clone).await;
        }
    });

    let body = ComputeHashResponse {
        status: "in_progress".to_string(),
        executable_hash: None,
        request_id: Some(request_id),
        message: "Build kicked off. Poll GET /job/:request_id for status, then \
                  GET /resolve-hash/:hash once it completes."
            .to_string(),
    };
    (StatusCode::ACCEPTED, Json(body.into()))
}
