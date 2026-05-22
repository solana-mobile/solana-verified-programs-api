//! Async program verification: `POST /verify` and `POST /verify-with-signer`.

use crate::{
    build::{self, resolve_build_params},
    db::{Db, NewBuild, BUILD_STATUS_COMPLETED, BUILD_STATUS_IN_PROGRESS},
    error::Result,
    response::VerifyResponse,
    types::{ProgramId, Signer, WebhookUrl},
};
use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use tracing::debug;

/// The actual build parameters come from the on-chain Otter Verify PDA;
/// any other body fields a caller sends are ignored.
#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub program_id: ProgramId,
    #[serde(default)]
    pub signer: Option<Signer>,
    #[serde(default)]
    pub webhook_url: Option<WebhookUrl>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyWithSignerRequest {
    pub signer: Signer,
    pub program_id: ProgramId,
    #[serde(default)]
    pub webhook_url: Option<WebhookUrl>,
}

pub async fn verify(
    State(db): State<Db>,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<VerifyResponse>)> {
    debug!("verify program={}", req.program_id);
    let (build_params, _signer, _state) =
        resolve_build_params(&req.program_id, req.signer.map(|s| s.0)).await?;
    let webhook = req.webhook_url.map(|w| w.into_inner());
    start_async(db, build_params, webhook).await
}

pub async fn verify_with_signer(
    State(db): State<Db>,
    Json(req): Json<VerifyWithSignerRequest>,
) -> Result<(StatusCode, Json<VerifyResponse>)> {
    debug!(
        "verify-with-signer program={} signer={}",
        req.program_id, req.signer
    );
    let (build_params, _signer, _state) =
        resolve_build_params(&req.program_id, Some(req.signer.0)).await?;
    let webhook = req.webhook_url.map(|w| w.into_inner());
    start_async(db, build_params, webhook).await
}

pub(super) async fn start_async(
    db: Db,
    params: NewBuild,
    webhook_url: Option<String>,
) -> Result<(StatusCode, Json<VerifyResponse>)> {
    if let Some(dup) = db.find_duplicate(&params).await? {
        return Ok((
            StatusCode::OK,
            Json(VerifyResponse {
                status: dup.status.clone(),
                request_id: dup.id.to_string(),
                message: match dup.status.as_str() {
                    BUILD_STATUS_IN_PROGRESS => "Build verification already in progress".into(),
                    BUILD_STATUS_COMPLETED => "Verification already completed.".into(),
                    _ => "Build record exists.".into(),
                },
            }),
        ));
    }

    let id = db.insert_build(&params).await?;
    let db2 = db.clone();
    let params2 = params.clone();
    tokio::spawn(async move {
        build::execute(id, params2, db2, webhook_url).await;
    });

    Ok((
        StatusCode::OK,
        Json(VerifyResponse {
            status: BUILD_STATUS_IN_PROGRESS.into(),
            request_id: id.to_string(),
            message: "Build verification started".into(),
        }),
    ))
}
