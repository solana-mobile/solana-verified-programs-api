use crate::{
    build::{self, resolve_build_params},
    db::{Db, BUILD_STATUS_COMPLETED, BUILD_STATUS_IN_PROGRESS},
    error::Result,
    onchain::build_repo_url,
    response::{StatusResponse, VerifyResponse},
    types::{ProgramId, Signer, WebhookUrl},
};
use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

/// The actual build parameters come from the on-chain Otter Verify PDA;
/// everything else the request body might carry (repository, commit_hash,
/// lib_name, base_image, mount_path, cargo_args, bpf_flag, arch) is ignored
/// by serde's default behaviour.
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

/// `POST /verify` — kick off an async build, return its job UUID.
pub async fn verify_async(
    State(db): State<Db>,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<VerifyResponse>)> {
    debug!("verify program={}", req.program_id);
    let (build_params, _signer, _state) =
        resolve_build_params(&req.program_id, req.signer.map(|s| s.0)).await?;
    let webhook = req.webhook_url.map(|w| w.into_inner());
    start_async(db, build_params, webhook).await
}

/// `POST /verify-with-signer` — async build pinned to a specific signer.
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

/// `POST /verify_sync` — runs the build inline.
///
/// Returns a [`StatusResponse`] for a fresh/completed build, or a
/// [`VerifyResponse`] when an in-progress duplicate is found — distinct
/// shapes, hence the `Value` return.
pub async fn verify_sync(
    State(db): State<Db>,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    debug!("verify_sync program={}", req.program_id);
    let (params, _signer, _state) =
        resolve_build_params(&req.program_id, req.signer.map(|s| s.0)).await?;

    if let Some(dup) = db.find_duplicate(&params).await? {
        match dup.status.as_str() {
            BUILD_STATUS_COMPLETED => {
                let resp = StatusResponse {
                    is_verified: matches_on_chain(
                        &db,
                        &dup.program_id,
                        dup.executable_hash.as_deref(),
                    )
                    .await,
                    message: "Verification already completed.".into(),
                    on_chain_hash: read_on_chain_hash(&db, &dup.program_id).await,
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
        message: if outcome.is_verified {
            "On chain program verified".into()
        } else {
            "On chain program not verified".into()
        },
        on_chain_hash: outcome.on_chain_hash,
        executable_hash: outcome.executable_hash,
        repo_url: build_repo_url(&params.repository, params.commit_hash.as_deref()),
        commit: params.commit_hash.clone().unwrap_or_default(),
        last_verified_at: Some(Utc::now().naive_utc()),
    };
    Ok((StatusCode::OK, Json(serde_json::to_value(resp).unwrap())))
}

async fn start_async(
    db: Db,
    params: crate::db::NewBuild,
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

async fn matches_on_chain(db: &Db, program_id: &str, exec_hash: Option<&str>) -> bool {
    let Some(exec) = exec_hash else { return false };
    let Ok(Some(state)) = db.get_program_state(program_id).await else {
        return false;
    };
    state.on_chain_hash.as_deref() == Some(exec)
}

async fn read_on_chain_hash(db: &Db, program_id: &str) -> String {
    db.get_program_state(program_id)
        .await
        .ok()
        .flatten()
        .and_then(|s| s.on_chain_hash)
        .unwrap_or_default()
}
