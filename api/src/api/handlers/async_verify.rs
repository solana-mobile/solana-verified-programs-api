use super::verify_helpers::{
    create_and_insert_build, setup_verification, validate_program_id, validate_repository_url,
    validate_signer, validate_webhook_url,
};
use crate::{
    db::{
        models::{
            ApiResponse, JobStatus, SolanaProgramBuildParams, SolanaProgramBuildParamsWithSigner,
            VerifyResponse,
        },
        DbClient,
    },
    services::verification::{notify_webhook, process_verification_request},
};
use axum::{extract::State, http::StatusCode, Json};
use tracing::{error, info};

/// Handler for asynchronous program verification
///
/// # Endpoint: POST /verify
pub(crate) async fn process_async_verification(
    State(db): State<DbClient>,
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
    State(db): State<DbClient>,
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

    match setup_verification(&db, &payload.program_id, Some(payload.signer)).await {
        Ok(setup) => {
            process_verification(db, setup.params, setup.signer, payload.webhook_url.clone()).await
        }
        Err(error_response) => error_response,
    }
}

/// Processes the verification request asynchronously
pub async fn process_verification(
    db: DbClient,
    payload: SolanaProgramBuildParams,
    signer: String,
    webhook_url: Option<String>,
) -> (StatusCode, Json<ApiResponse>) {
    // Content-addressed fast path: if anyone has already built this exact
    // (repo, commit, build_args), the hash is in the directory. Skip the
    // build and return the hash inline.
    if let Ok(Some(cached)) = db.find_hash_for_build_params(&payload).await {
        info!(
            "Directory cache hit for {}: returning hash {} without rebuilding",
            payload.program_id, cached.executable_hash
        );
        return (
            StatusCode::OK,
            Json(
                VerifyResponse {
                    status: JobStatus::Completed,
                    request_id: String::new(),
                    message: format!(
                        "Build already in the directory; cached executable_hash: {}",
                        cached.executable_hash
                    ),
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
    spawn_verification_task(db.clone(), payload, verification_uuid.clone(), webhook_url).await;

    // Return response with request ID
    (
        StatusCode::OK,
        Json(
            VerifyResponse {
                status: JobStatus::InProgress,
                request_id: verification_uuid,
                message: "Build verification started".to_string(),
            }
            .into(),
        ),
    )
}

/// Spawns an asynchronous verification task
async fn spawn_verification_task(
    db: DbClient,
    payload: SolanaProgramBuildParams,
    uuid: String,
    webhook_url: Option<String>,
) {
    info!("Verification task spawned with UUID: {}", uuid);
    tokio::spawn(async move {
        info!("Spawning verification task with uuid: {}", uuid);
        let result = process_verification_request(payload, &uuid, &db).await;
        if let Err(e) = &result {
            error!("Verification task failed: {:?}", e);
        }
        if let Some(url) = webhook_url {
            notify_webhook(url, result, uuid).await;
        }
    });
}

