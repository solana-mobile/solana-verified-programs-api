use crate::db::models::{
    ApiResponse, ErrorResponse, ExtendedStatusResponse, Status, StatusResponse, SuccessResponse,
    VerificationStatusParams,
};
use crate::db::DbClient;
use crate::services::get_on_chain_hash;
use crate::services::onchain::get_program_authority;
use crate::validation;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::{error, info};

/// Handler for checking if a specific program is verified.
///
/// # Endpoint: GET /status/:address
///
/// In the content-addressed model this is a thin wrapper:
///   1. Fetch the current on-chain program hash via RPC.
///   2. Look that hash up in the `verified_hashes` directory.
///   3. Return verified iff the directory has an entry for the hash.
///
/// No `is_verified` flag, no invalidation, no staleness — the answer is live
/// because every call hashes whatever is currently on-chain.
pub(crate) async fn get_verification_status(
    State(db): State<DbClient>,
    Path(VerificationStatusParams { address }): Path<VerificationStatusParams>,
) -> (StatusCode, Json<ExtendedStatusResponse>) {
    if let Err(e) = validation::validate_pubkey(&address) {
        return (
            StatusCode::BAD_REQUEST,
            Json(not_verified_response(
                e,
                String::new(),
                None,
                false,
                false,
            )),
        );
    }

    info!("Checking verification status for program: {}", address);

    // Authority info (frozen/closed). Failures default to "unknown" = (false, false).
    let (is_frozen, is_closed) = match get_program_authority(&address).await {
        Ok((_, frozen, closed)) => (frozen, closed),
        Err(_) => (false, false),
    };

    if is_closed {
        return (
            StatusCode::OK,
            Json(not_verified_response(
                "Program is closed".to_string(),
                String::new(),
                None,
                is_frozen,
                true,
            )),
        );
    }

    // Fetch current on-chain hash.
    let on_chain_hash = match get_on_chain_hash(&address).await {
        Ok(hash) => hash,
        Err(e) => {
            error!("Failed to fetch on-chain hash for {}: {}", address, e);
            return (
                StatusCode::OK,
                Json(not_verified_response(
                    "Failed to fetch on-chain hash".to_string(),
                    String::new(),
                    None,
                    is_frozen,
                    is_closed,
                )),
            );
        }
    };

    // Directory lookup: does any verified build produce these bytes?
    match db.get_verified_hash(&on_chain_hash).await {
        Ok(Some(entry)) => {
            info!(
                "Program {} verified: hash {} resolves to repo {}",
                address, on_chain_hash, entry.repository
            );
            let commit = entry.commit_hash.clone().unwrap_or_default();
            let repo_url = if commit.is_empty() {
                entry.repository.clone()
            } else {
                format!("{}/tree/{}", entry.repository.trim_end_matches('/'), commit)
            };
            let verified_at = Some(entry.verified_at);
            (
                StatusCode::OK,
                Json(ExtendedStatusResponse {
                    status: StatusResponse {
                        is_verified: true,
                        message: "On chain program verified".to_string(),
                        on_chain_hash: on_chain_hash.clone(),
                        executable_hash: on_chain_hash,
                        repo_url,
                        commit,
                        last_verified_at: verified_at,
                    },
                    is_frozen,
                    is_closed,
                }),
            )
        }
        Ok(None) => (
            StatusCode::OK,
            Json(not_verified_response(
                "On chain program not verified".to_string(),
                on_chain_hash,
                None,
                is_frozen,
                is_closed,
            )),
        ),
        Err(e) => {
            error!("Directory lookup failed for hash {}: {:?}", on_chain_hash, e);
            (
                StatusCode::OK,
                Json(not_verified_response(
                    "Directory lookup failed".to_string(),
                    on_chain_hash,
                    None,
                    is_frozen,
                    is_closed,
                )),
            )
        }
    }
}

fn not_verified_response(
    message: String,
    on_chain_hash: String,
    last_verified_at: Option<chrono::NaiveDateTime>,
    is_frozen: bool,
    is_closed: bool,
) -> ExtendedStatusResponse {
    ExtendedStatusResponse {
        status: StatusResponse {
            is_verified: false,
            message,
            on_chain_hash,
            executable_hash: String::new(),
            repo_url: String::new(),
            commit: String::new(),
            last_verified_at,
        },
        is_frozen,
        is_closed,
    }
}

/// Handler for retrieving all verification information for a program.
///
/// # Endpoint: GET /status-all/:address
pub(crate) async fn get_verification_status_all(
    State(db): State<DbClient>,
    Path(VerificationStatusParams { address }): Path<VerificationStatusParams>,
) -> (StatusCode, Json<ApiResponse>) {
    if let Err(e) = validation::validate_pubkey(&address) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                ErrorResponse {
                    status: Status::Error,
                    error: e,
                }
                .into(),
            ),
        );
    }

    info!(
        "Fetching all verification information for program: {}",
        address
    );

    match db.get_all_verification_info(address).await {
        Ok(result) => {
            info!("Successfully retrieved all verification info");
            (
                StatusCode::OK,
                Json(ApiResponse::Success(SuccessResponse::StatusAll(result))),
            )
        }
        Err(err) => {
            error!(
                "Failed to get verification information from database: {}",
                err
            );
            (
                StatusCode::OK,
                Json(
                    ErrorResponse {
                        status: Status::Error,
                        error: "An unexpected database error occurred.".to_string(),
                    }
                    .into(),
                ),
            )
        }
    }
}
