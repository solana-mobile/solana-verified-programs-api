//! Synchronous program verification: `POST /verify_sync`.

use crate::{
    build::{self, resolve_build_params},
    db::{Db, BUILD_STATUS_COMPLETED, BUILD_STATUS_IN_PROGRESS},
    error::Result,
    handlers::async_verify::VerifyRequest,
    handlers::status_message,
    onchain::build_repo_url,
    response::{StatusResponse, VerifyResponse},
};
use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde_json::Value;
use tracing::debug;

/// Returns a [`StatusResponse`] for a fresh/completed build, or a
/// [`VerifyResponse`] when an in-progress duplicate is found — those shapes
/// are distinct, hence the `Value` return.
pub async fn process_sync_verification(
    State(db): State<Db>,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    debug!("verify_sync program={}", req.program_id);
    let (params, _signer, _state) =
        resolve_build_params(&req.program_id, req.signer.map(|s| s.0)).await?;

    if let Some(dup) = db.find_duplicate(&params).await? {
        match dup.status.as_str() {
            BUILD_STATUS_COMPLETED => {
                let on_chain_hash = db
                    .get_program_state(&dup.program_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| s.on_chain_hash)
                    .unwrap_or_default();
                let is_verified = !on_chain_hash.is_empty()
                    && dup.executable_hash.as_deref() == Some(on_chain_hash.as_str());
                let resp = StatusResponse {
                    is_verified,
                    message: "Verification already completed.".into(),
                    on_chain_hash,
                    executable_hash: dup.executable_hash.unwrap_or_default(),
                    repo_url: build_repo_url(&dup.repository, dup.commit_hash.as_deref()),
                    commit: dup.commit_hash.unwrap_or_default(),
                    last_verified_at: dup.completed_at.map(|t| t.naive_utc()),
                };
                return Ok((StatusCode::OK, Json(serde_json::to_value(resp).unwrap())));
            }
            BUILD_STATUS_IN_PROGRESS => {
                let resp = VerifyResponse {
                    status: BUILD_STATUS_IN_PROGRESS.into(),
                    request_id: dup.id.to_string(),
                    message: "Build verification already in progress".into(),
                };
                return Ok((StatusCode::OK, Json(serde_json::to_value(resp).unwrap())));
            }
            _ => {}
        }
    }

    let id = db.insert_build(&params).await?;
    let outcome = build::run_build(id, &params, &db).await?;
    build::finalize_completed(&db, id, &outcome, &params.program_id).await;
    let resp = StatusResponse {
        is_verified: outcome.is_verified,
        message: status_message(outcome.is_verified),
        on_chain_hash: outcome.on_chain_hash,
        executable_hash: outcome.executable_hash,
        repo_url: build_repo_url(&params.repository, params.commit_hash.as_deref()),
        commit: params.commit_hash.clone().unwrap_or_default(),
        last_verified_at: Some(Utc::now().naive_utc()),
    };
    Ok((StatusCode::OK, Json(serde_json::to_value(resp).unwrap())))
}
