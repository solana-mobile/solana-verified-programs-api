//! Shared verification helpers and utilities
//! Contains common logic used across different verification endpoints

use crate::{
    db::{
        models::{
            ApiResponse, ErrorResponse, SolanaProgramBuild, SolanaProgramBuildParams, Status,
        },
        DbClient,
    },
    errors::ErrorMessages,
    services::onchain::{self, get_program_authority},
    validation,
};
use axum::{http::StatusCode, Json};
use tracing::{error, info};


/// Result type for verification setup operations
pub type VerificationSetupResult = Result<VerificationSetup, (StatusCode, Json<ApiResponse>)>;

/// Contains all the setup data needed for verification
pub struct VerificationSetup {
    pub params: SolanaProgramBuildParams,
    pub signer: String,
}

/// Common setup logic for verification endpoints: fetch the on-chain program
/// authority (hint for picking the PDA signer), then read the Otter verify
/// PDA to get the canonical build params + signer.
pub async fn setup_verification(
    program_id: &str,
    specific_signer: Option<String>,
) -> VerificationSetupResult {
    let program_authority = match get_program_authority(program_id).await {
        Ok((authority, _frozen, _closed)) => authority,
        Err(e) => {
            info!("Failed to fetch program authority for {}: {}", program_id, e);
            None
        }
    };

    match onchain::get_otter_verify_params(program_id, specific_signer, program_authority).await {
        Ok((params, signer)) => Ok(VerificationSetup {
            params: SolanaProgramBuildParams::from(params),
            signer,
        }),
        Err(err) => {
            error!(
                "Unable to find on-chain PDA for program {}: {:?}",
                program_id, err
            );
            Err(create_not_found_error())
        }
    }
}

/// Creates a standardized "not found" error response for missing PDAs
pub fn create_not_found_error() -> (StatusCode, Json<ApiResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(
            ErrorResponse {
                status: Status::Error,
                error: ErrorMessages::NoPDA.to_string(),
            }
            .into(),
        ),
    )
}

/// Creates a standardized database error response
pub fn create_db_error() -> (StatusCode, Json<ApiResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(
            ErrorResponse {
                status: Status::Error,
                error: ErrorMessages::DB.to_string(),
            }
            .into(),
        ),
    )
}

/// Creates a standardized internal server error response
pub fn create_internal_error() -> (StatusCode, Json<ApiResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(
            ErrorResponse {
                status: Status::Error,
                error: ErrorMessages::Unexpected.to_string(),
            }
            .into(),
        ),
    )
}

/// Creates a 400 Bad Request response for input validation errors (invalid pubkey, URL, etc.)
pub fn validation_error_response(message: impl Into<String>) -> (StatusCode, Json<ApiResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(
            ErrorResponse {
                status: Status::Error,
                error: message.into(),
            }
            .into(),
        ),
    )
}

/// Validation helpers for verification endpoints
pub type HandlerError = (StatusCode, Json<ApiResponse>);
pub type HandlerResult<T> = Result<T, HandlerError>;

#[allow(clippy::result_large_err)]
pub fn validate_pubkey(value: &str) -> HandlerResult<()> {
    validation::validate_pubkey(value)
        .map(|_| ())
        .map_err(validation_error_response)
}

#[allow(clippy::result_large_err)]
pub fn validate_http_url(value: &str) -> HandlerResult<()> {
    validation::validate_http_url(value).map_err(validation_error_response)
}

#[allow(clippy::result_large_err)]
pub fn validate_program_id(program_id: &str) -> HandlerResult<()> {
    validate_pubkey(program_id)
}

#[allow(clippy::result_large_err)]
pub fn validate_signer(signer: &str) -> HandlerResult<()> {
    validate_pubkey(signer)
}

#[allow(clippy::result_large_err)]
pub fn validate_repository_url(repository: &str) -> HandlerResult<()> {
    validate_http_url(repository)
}

#[allow(clippy::result_large_err)]
pub fn validate_webhook_url(webhook_url: &Option<String>) -> HandlerResult<()> {
    if let Some(url) = webhook_url.as_deref() {
        validate_http_url(url)?;
    }
    Ok(())
}

/// Creates and inserts build parameters into the database
/// Returns the UUID of the created build
pub async fn create_and_insert_build(
    db: &DbClient,
    params: &SolanaProgramBuildParams,
    signer: &str,
) -> Result<String, (StatusCode, Json<ApiResponse>)> {
    let mut build_data = SolanaProgramBuild::from(params);
    build_data.signer = Some(signer.to_string());
    let uuid = build_data.id.clone();

    if let Err(e) = db.insert_build_params(&build_data).await {
        error!("Error inserting build parameters: {:?}", e);
        return Err(create_db_error());
    }

    Ok(uuid)
}
