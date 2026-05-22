//! Asynchronous program verification endpoints: `POST /verify` and `/verify-with-signer`.

use crate::{
    build,
    db::{BUILD_STATUS_COMPLETED, BUILD_STATUS_IN_PROGRESS, Db, NewBuild},
    error::Result,
    handlers::verify_helpers::{
        create_and_insert_build, setup_verification, validate_program_id, validate_signer,
        validate_webhook_url,
    },
    response::VerifyResponse,
};
use axum::{Json, extract::State};
use serde::Deserialize;
use tracing::info;

/// Request body for `POST /verify`. Build params come from the on-chain
/// Otter Verify PDA; the legacy `repository`/`commit_hash`/etc. fields are
/// accepted for backward compatibility but ignored.
#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub program_id: String,
    #[serde(default)]
    pub webhook_url: Option<String>,
}

/// Request body for `POST /verify-with-signer`. Pins which signer's PDA
/// claim to use.
#[derive(Debug, Deserialize)]
pub struct VerifyWithSignerRequest {
    pub program_id: String,
    pub signer: String,
    #[serde(default)]
    pub webhook_url: Option<String>,
}

/// Handler for asynchronous program verification
///
/// # Endpoint: POST /verify
pub async fn verify(
    State(db): State<Db>,
    Json(payload): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>> {
    let program_id = validate_program_id(&payload.program_id)?;
    validate_webhook_url(&payload.webhook_url)?;

    info!("Starting async verification for program: {}", program_id);

    let (params, _signer) = setup_verification(&db, &program_id, None).await?;
    process_verification(db, params, payload.webhook_url).await
}

/// Handler for asynchronous program verification with a specific signer
///
/// # Endpoint: POST /verify-with-signer
pub async fn verify_with_signer(
    State(db): State<Db>,
    Json(payload): Json<VerifyWithSignerRequest>,
) -> Result<Json<VerifyResponse>> {
    let program_id = validate_program_id(&payload.program_id)?;
    let signer = validate_signer(&payload.signer)?;
    validate_webhook_url(&payload.webhook_url)?;

    info!(
        "Starting async verification for program {} with signer {}",
        program_id, signer
    );

    let (params, _signer) = setup_verification(&db, &program_id, Some(signer)).await?;
    process_verification(db, params, payload.webhook_url).await
}

async fn process_verification(
    db: Db,
    params: NewBuild,
    webhook_url: Option<String>,
) -> Result<Json<VerifyResponse>> {
    // If we've already seen identical params, surface the existing row.
    if let Some(dup) = db.find_duplicate(&params).await? {
        return Ok(Json(VerifyResponse {
            status: dup.status.clone(),
            request_id: dup.id.to_string(),
            message: match dup.status.as_str() {
                BUILD_STATUS_IN_PROGRESS => "Build verification already in progress".into(),
                BUILD_STATUS_COMPLETED => "Verification already completed.".into(),
                _ => "Build record exists.".into(),
            },
        }));
    }

    let id = create_and_insert_build(&db, &params).await?;

    let db_clone = db.clone();
    let params_clone = params.clone();
    tokio::spawn(async move {
        build::execute(id, params_clone, db_clone, webhook_url).await;
    });

    Ok(Json(VerifyResponse {
        status: BUILD_STATUS_IN_PROGRESS.into(),
        request_id: id.to_string(),
        message: "Build verification started".into(),
    }))
}
