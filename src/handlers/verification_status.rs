//! `GET /status/:address` and `GET /status-all/:address`.

use crate::{
    db::{BuildRow, Db},
    error::Result,
    handlers::status_message,
    onchain::build_repo_url,
    response::{
        ExtendedStatusResponse, StatusResponse, VerificationResponse,
        VerificationResponseWithSigner,
    },
    types::ProgramId,
};
use axum::{
    extract::{Path, State},
    Json,
};

pub async fn status(
    State(db): State<Db>,
    Path(program_id): Path<ProgramId>,
) -> Json<ExtendedStatusResponse> {
    let state = db
        .get_program_state(&program_id.as_str())
        .await
        .ok()
        .flatten();
    let on_chain_hash = state
        .as_ref()
        .and_then(|s| s.on_chain_hash.clone())
        .unwrap_or_default();
    let is_frozen = state.as_ref().is_some_and(|s| s.is_frozen.unwrap_or(false));
    let is_closed = state.as_ref().is_some_and(|s| s.is_closed);
    // best_build needs the on-chain hash to break ties when multiple
    // completed builds exist (post-upgrade history). Sequential is the
    // simplest correct shape.
    let build = db
        .best_build(&program_id, Some(on_chain_hash.as_str()))
        .await
        .ok()
        .flatten();

    let status = match build {
        Some(b) => {
            let is_verified = !on_chain_hash.is_empty()
                && b.executable_hash.as_deref() == Some(on_chain_hash.as_str())
                && !is_closed;
            StatusResponse {
                is_verified,
                message: status_message(is_verified),
                on_chain_hash,
                executable_hash: b.executable_hash.unwrap_or_default(),
                repo_url: build_repo_url(&b.repository, b.commit_hash.as_deref()),
                commit: b.commit_hash.unwrap_or_default(),
                last_verified_at: b.completed_at.map(|t| t.naive_utc()),
            }
        }
        None => StatusResponse {
            is_verified: false,
            message: "On chain program not verified".into(),
            on_chain_hash,
            executable_hash: String::new(),
            repo_url: String::new(),
            commit: String::new(),
            last_verified_at: None,
        },
    };
    Json(ExtendedStatusResponse {
        status,
        is_frozen,
        is_closed,
    })
}

pub async fn status_all(
    State(db): State<Db>,
    Path(program_id): Path<ProgramId>,
) -> Result<Json<Vec<VerificationResponseWithSigner>>> {
    let state = db.get_program_state(&program_id.as_str()).await?;
    let on_chain_hash = state
        .as_ref()
        .and_then(|s| s.on_chain_hash.clone())
        .unwrap_or_default();
    let is_frozen = state.as_ref().is_some_and(|s| s.is_frozen.unwrap_or(false));
    let is_closed = state.as_ref().is_some_and(|s| s.is_closed);

    let builds = db.completed_builds_by_signer(&program_id).await?;
    let out = builds
        .into_iter()
        .map(|b| build_with_signer(b, &on_chain_hash, is_frozen, is_closed))
        .collect();
    Ok(Json(out))
}

fn build_with_signer(
    b: BuildRow,
    on_chain_hash: &str,
    is_frozen: bool,
    is_closed: bool,
) -> VerificationResponseWithSigner {
    let is_verified = !on_chain_hash.is_empty()
        && b.executable_hash.as_deref() == Some(on_chain_hash)
        && !is_closed;
    VerificationResponseWithSigner {
        signer: b.signer.clone().unwrap_or_default(),
        verification_response: VerificationResponse {
            is_verified,
            on_chain_hash: on_chain_hash.to_string(),
            executable_hash: b.executable_hash.unwrap_or_default(),
            repo_url: build_repo_url(&b.repository, b.commit_hash.as_deref()),
            commit: b.commit_hash.unwrap_or_default(),
            last_verified_at: b.completed_at.map(|t| t.naive_utc()),
            is_frozen,
            is_closed,
        },
    }
}
