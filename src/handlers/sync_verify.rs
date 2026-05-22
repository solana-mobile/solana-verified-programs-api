use super::async_verify::SolanaProgramBuildParams;
use super::verify_helpers::{
    HandlerError, create_and_insert_build, setup_verification, validate_program_id,
    validate_repository_url,
};
use crate::{
    build,
    db::{Db, NewBuild},
    onchain::build_repo_url,
    response::{ApiResponse, ErrorResponse, Status, StatusResponse},
};
use axum::{Json, extract::State, http::StatusCode};
use chrono::Utc;
use tracing::{error, info};

/// Handler for synchronous program verification
///
/// # Endpoint: POST /verify_sync
///
/// # Arguments
/// * `db` - Database client from application state
/// * `payload` - Build parameters for verification
///
/// # Returns
/// * `(StatusCode, Json<ApiResponse>)` - Status code and verification response
///
/// This endpoint performs verification synchronously, meaning it will wait for
/// the verification process to complete before returning a response.
pub(crate) async fn process_sync_verification(
    State(db): State<Db>,
    Json(payload): Json<SolanaProgramBuildParams>,
) -> (StatusCode, Json<ApiResponse>) {
    if let Err(resp) = validate_program_id(&payload.program_id) {
        return resp;
    }
    if let Err(resp) = validate_repository_url(&payload.repository) {
        return resp;
    }

    info!(
        "Starting synchronous verification for program: {}",
        payload.program_id
    );

    match setup_verification(&db, &payload.program_id, None).await {
        Ok(setup) => process_verification_sync(db, setup.params, setup.signer).await,
        Err(error_response) => error_response,
    }
}

/// Processes the verification request synchronously
async fn process_verification_sync(
    db: Db,
    params: NewBuild,
    signer: String,
) -> (StatusCode, Json<ApiResponse>) {
    if let Ok(Some(dup)) = db.find_duplicate(&params).await {
        return (
            StatusCode::OK,
            Json(
                crate::response::VerifyResponse {
                    status: dup.status.clone(),
                    request_id: dup.id.to_string(),
                    message: "Verification already completed.".to_string(),
                }
                .into(),
            ),
        );
    }

    let uuid = match create_and_insert_build(&db, &params, &signer).await {
        Ok(uuid) => uuid,
        Err(error_response) => return error_response,
    };

    match build::run_build(uuid, &params, &db).await {
        Ok(outcome) => {
            build::finalize_completed(&db, uuid, &outcome, &params.program_id).await;
            info!(
                "Verification completed for program: {} (verified: {})",
                params.program_id, outcome.is_verified
            );

            (
                StatusCode::OK,
                Json(
                    StatusResponse {
                        is_verified: outcome.is_verified,
                        message: if outcome.is_verified {
                            "On chain program verified"
                        } else {
                            "On chain program not verified"
                        }
                        .to_string(),
                        on_chain_hash: outcome.on_chain_hash,
                        executable_hash: outcome.executable_hash,
                        last_verified_at: Some(Utc::now().naive_utc()),
                        repo_url: build_repo_url(&params.repository, params.commit_hash.as_deref()),
                        commit: params.commit_hash.clone().unwrap_or_default(),
                    }
                    .into(),
                ),
            )
        }
        Err(err) => {
            error!("Verification failed: {:?}", err);
            create_internal_error()
        }
    }
}

fn create_internal_error() -> HandlerError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(
            ErrorResponse {
                status: Status::Error,
                error: "An unexpected error occurred.".to_string(),
            }
            .into(),
        ),
    )
}
