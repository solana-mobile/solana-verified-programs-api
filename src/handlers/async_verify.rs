use super::verify_helpers::{
    create_and_insert_build, setup_verification, validate_program_id, validate_repository_url,
    validate_signer, validate_webhook_url,
};
use crate::{
    build,
    db::{BUILD_STATUS_COMPLETED, BUILD_STATUS_IN_PROGRESS, Db, NewBuild},
    onchain::is_program_buffer_missing,
    response::{ApiResponse, VerifyResponse},
};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use tracing::{error, info};
use uuid::Uuid;

/// Request body for `POST /verify`. Build params come from the on-chain
/// Otter Verify PDA; the legacy `repository`/`commit_hash`/etc. fields are
/// accepted for backward compatibility but ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct SolanaProgramBuildParams {
    pub repository: String,
    pub program_id: String,
    #[serde(default)]
    pub webhook_url: Option<String>,
}

/// Request body for `POST /verify-with-signer`.
#[derive(Debug, Clone, Deserialize)]
pub struct SolanaProgramBuildParamsWithSigner {
    pub program_id: String,
    pub signer: String,
    #[serde(default)]
    pub webhook_url: Option<String>,
}

/// Handler for asynchronous program verification
///
/// # Endpoint: POST /verify
pub(crate) async fn process_async_verification(
    State(db): State<Db>,
    Json(payload): Json<SolanaProgramBuildParams>,
) -> (StatusCode, Json<ApiResponse>) {
    if let Err(resp) = validate_program_id(&payload.program_id) {
        return resp;
    }
    if let Err(resp) = validate_repository_url(&payload.repository) {
        return resp;
    }
    if let Err(resp) = validate_webhook_url(&payload.webhook_url) {
        return resp;
    }

    info!(
        "Starting async verification for program: {}",
        payload.program_id
    );

    match setup_verification(&db, &payload.program_id, None).await {
        Ok(setup) => {
            process_verification(db, setup.params, setup.signer, payload.webhook_url.clone()).await
        }
        Err(error_response) => error_response,
    }
}

/// Handler for asynchronous program verification with a specific signer
///
/// # Endpoint: POST /verify-with-signer
pub(crate) async fn process_async_verification_with_signer(
    State(db): State<Db>,
    Json(payload): Json<SolanaProgramBuildParamsWithSigner>,
) -> (StatusCode, Json<ApiResponse>) {
    if let Err(resp) = validate_program_id(&payload.program_id) {
        return resp;
    }
    if let Err(resp) = validate_signer(&payload.signer) {
        return resp;
    }
    if let Err(resp) = validate_webhook_url(&payload.webhook_url) {
        return resp;
    }

    info!(
        "Starting async verification for program {} with signer {}",
        payload.program_id, payload.signer
    );

    match setup_verification(&db, &payload.program_id, Some(payload.signer.clone())).await {
        Ok(setup) => {
            process_verification(db, setup.params, setup.signer, payload.webhook_url.clone()).await
        }
        Err(error_response) => error_response,
    }
}

/// Processes the verification request asynchronously
pub async fn process_verification(
    db: Db,
    payload: NewBuild,
    signer: String,
    webhook_url: Option<String>,
) -> (StatusCode, Json<ApiResponse>) {
    // Check for existing verification
    if let Ok(Some(dup)) = db.find_duplicate(&payload).await {
        check_program_closed(&db, &payload.program_id).await;
        return (
            StatusCode::OK,
            Json(
                VerifyResponse {
                    status: dup.status.clone(),
                    request_id: dup.id.to_string(),
                    message: match dup.status.as_str() {
                        BUILD_STATUS_IN_PROGRESS => "Build verification already in progress".into(),
                        BUILD_STATUS_COMPLETED => "Verification already completed.".into(),
                        _ => "Build record exists.".into(),
                    },
                }
                .into(),
            ),
        );
    }

    // Create build record for the verification
    let verification_uuid = match create_and_insert_build(&db, &payload, &signer).await {
        Ok(uuid) => uuid,
        Err(error_response) => return error_response,
    };

    // Spawn async verification task
    spawn_verification_task(db.clone(), payload, verification_uuid, webhook_url).await;

    // Return response with request ID
    (
        StatusCode::OK,
        Json(
            VerifyResponse {
                status: BUILD_STATUS_IN_PROGRESS.to_string(),
                request_id: verification_uuid.to_string(),
                message: "Build verification started".to_string(),
            }
            .into(),
        ),
    )
}

/// Spawns an asynchronous verification task
async fn spawn_verification_task(
    db: Db,
    payload: NewBuild,
    uuid: Uuid,
    webhook_url: Option<String>,
) {
    info!("Verification task spawned with UUID: {}", uuid);
    tokio::spawn(async move {
        info!("Spawning verification task with uuid: {}", uuid);
        build::execute(uuid, payload, db, webhook_url).await;
    });
}

/// Checks if the program's buffer account is missing, and if so,
/// marks the program as unverified in the database.
pub async fn check_program_closed(db: &Db, program_id: &str) {
    let Ok(pid) = Pubkey::from_str(program_id) else {
        return;
    };
    if is_program_buffer_missing(&pid).await {
        info!(
            "Program {} buffer missing. Marking as unverified.",
            program_id
        );

        if let Err(e) = db.mark_closed(program_id).await {
            error!(
                "Program {} buffer missing. failed to mark as unverified: {:?}",
                program_id, e
            );
        }
    }
}
