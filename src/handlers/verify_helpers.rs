//! Shared verification helpers and utilities
//! Contains common logic used across different verification endpoints

use crate::{
    db::{Db, NewBuild},
    onchain::{self, OtterBuildParams},
    response::{ApiResponse, ErrorResponse, Status},
    validation,
};
use axum::{Json, http::StatusCode};
use solana_pubkey::Pubkey;
use std::str::FromStr;
use tracing::error;
use uuid::Uuid;

/// Result type for verification setup operations
pub type VerificationSetupResult = Result<VerificationSetup, (StatusCode, Json<ApiResponse>)>;

/// Contains all the setup data needed for verification
pub struct VerificationSetup {
    pub params: NewBuild,
    pub signer: String,
}

/// Common setup logic for verification endpoints
///
/// Handles:
/// - Getting program authority and on-chain hash from on-chain
/// - Fetching verification parameters from PDA
/// - Updating program state in database
pub async fn setup_verification(
    db: &Db,
    program_id: &str,
    specific_signer: Option<String>,
) -> VerificationSetupResult {
    let pid = match Pubkey::from_str(program_id) {
        Ok(p) => p,
        Err(e) => {
            return Err(validation_error_response(format!(
                "Invalid program id: {e}"
            )));
        }
    };

    let state = onchain::get_program_state(&pid)
        .await
        .unwrap_or(onchain::ProgramOnchainState {
            authority: None,
            is_frozen: false,
            is_closed: false,
            executable_hash: None,
        });

    let specific = specific_signer
        .as_deref()
        .map(Pubkey::from_str)
        .transpose()
        .map_err(|e| validation_error_response(format!("Invalid signer: {e}")))?;

    match onchain::get_otter_verify_params(&pid, specific, state.authority.as_deref()).await {
        Ok((params, signer)) => {
            if let Err(e) = db
                .upsert_program_state(&params.address.to_string(), &state)
                .await
            {
                error!("Failed to update program state: {:?}", e);
            }

            Ok(VerificationSetup {
                params: NewBuild::from(&params),
                signer,
            })
        }
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
                error: "Otter Verify PDA not found".to_string(),
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
                error: "Unexpected database error".to_string(),
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

pub fn validate_pubkey(value: &str) -> HandlerResult<()> {
    validation::validate_pubkey(value)
        .map(|_| ())
        .map_err(validation_error_response)
}

pub fn validate_http_url(value: &str) -> HandlerResult<()> {
    validation::validate_http_url(value).map_err(validation_error_response)
}

pub fn validate_program_id(program_id: &str) -> HandlerResult<()> {
    validate_pubkey(program_id)
}

pub fn validate_signer(signer: &str) -> HandlerResult<()> {
    validate_pubkey(signer)
}

pub fn validate_repository_url(repository: &str) -> HandlerResult<()> {
    validate_http_url(repository)
}

pub fn validate_webhook_url(webhook_url: &Option<String>) -> HandlerResult<()> {
    if let Some(url) = webhook_url.as_deref() {
        validate_http_url(url)?;
    }
    Ok(())
}

/// Creates and inserts build parameters into the database
/// Returns the UUID of the created build
pub async fn create_and_insert_build(
    db: &Db,
    params: &NewBuild,
    signer: &str,
) -> Result<Uuid, (StatusCode, Json<ApiResponse>)> {
    let mut build = params.clone();
    build.signer = Some(signer.to_string());
    db.insert_build(&build).await.map_err(|e| {
        error!("Error inserting build parameters: {:?}", e);
        create_db_error()
    })
}

impl From<&OtterBuildParams> for NewBuild {
    fn from(p: &OtterBuildParams) -> Self {
        NewBuild {
            repository: p.git_url.clone(),
            commit_hash: Some(p.commit.clone()),
            program_id: p.address.to_string(),
            lib_name: p.library_name(),
            base_docker_image: p.base_image(),
            mount_path: p.mount_path(),
            cargo_args: p.cargo_args(),
            bpf_flag: p.bpf(),
            arch: p.arch(),
            signer: None,
        }
    }
}
