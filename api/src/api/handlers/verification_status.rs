use crate::db::models::{
    ApiResponse, ErrorResponse, ExtendedStatusResponse, Status, StatusResponse, SuccessResponse,
    VerificationResponse, VerificationResponseWithSigner, VerificationStatusParams, DEFAULT_SIGNER,
};
use crate::db::DbClient;
use crate::services::get_on_chain_hash;
use crate::services::onchain::{get_program_authority, program_metadata_retriever::SIGNER_KEYS};
use crate::validation;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::{error, info};

/// `GET /status/:address` — thin wrapper over the content-addressed directory.
///
///   1. RPC-fetch the current on-chain program hash.
///   2. Look that hash up in `verified_hashes` filtered by the trust set
///      `{program upgrade authority} ∪ SIGNER_KEYS ∪ {DEFAULT_SIGNER}`.
///   3. Return verified iff a trusted signer has claimed this hash.
///
/// No `is_verified` flag, no invalidation, no staleness — every call hashes
/// whatever is currently on-chain.
pub(crate) async fn get_verification_status(
    State(db): State<DbClient>,
    Path(VerificationStatusParams { address }): Path<VerificationStatusParams>,
) -> (StatusCode, Json<ExtendedStatusResponse>) {
    if let Err(e) = validation::validate_pubkey(&address) {
        return (
            StatusCode::BAD_REQUEST,
            Json(not_verified(e, String::new(), None, false, false)),
        );
    }

    info!("Checking verification status for program: {}", address);

    let (program_authority, is_frozen, is_closed) = match get_program_authority(&address).await {
        Ok((authority, frozen, closed)) => (authority, frozen, closed),
        Err(_) => (None, false, false),
    };

    if is_closed {
        return (
            StatusCode::OK,
            Json(not_verified(
                "Program is closed".to_string(),
                String::new(),
                None,
                is_frozen,
                true,
            )),
        );
    }

    let on_chain_hash = match get_on_chain_hash(&address).await {
        Ok(hash) => hash,
        Err(e) => {
            error!("Failed to fetch on-chain hash for {}: {}", address, e);
            return (
                StatusCode::OK,
                Json(not_verified(
                    "Failed to fetch on-chain hash".to_string(),
                    String::new(),
                    None,
                    is_frozen,
                    is_closed,
                )),
            );
        }
    };

    let trust_set = trust_set_for(program_authority.as_deref());
    match db
        .get_verified_hashes_trusted(&on_chain_hash, &trust_set)
        .await
    {
        Ok(rows) if !rows.is_empty() => {
            let entry = rows.into_iter().next().expect("non-empty");
            let commit = entry.commit_hash.unwrap_or_default();
            let repo_url = if commit.is_empty() {
                entry.repository
            } else {
                format!("{}/tree/{}", entry.repository.trim_end_matches('/'), commit)
            };
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
                        last_verified_at: Some(entry.verified_at),
                    },
                    is_frozen,
                    is_closed,
                }),
            )
        }
        Ok(_) => (
            StatusCode::OK,
            Json(not_verified(
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
                Json(not_verified(
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

/// Trust ordering matching the pre-content-addressing query:
/// `{program_authority} ∪ SIGNER_KEYS ∪ {DEFAULT_SIGNER}`. The system-program
/// sentinel is kept because the Solana Explorer treats it as a trusted claim
/// and legacy rows were defaulted to it.
fn trust_set_for(program_authority: Option<&str>) -> Vec<String> {
    let mut out = Vec::with_capacity(2 + SIGNER_KEYS.len());
    if let Some(a) = program_authority {
        out.push(a.to_string());
    }
    out.extend(SIGNER_KEYS.iter().map(|k| k.to_string()));
    out.push(DEFAULT_SIGNER.to_string());
    out
}

fn not_verified(
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

/// `GET /status-all/:address` — every trusted signer's claim for the
/// program's current on-chain hash. Internally:
///
///   1. RPC-fetch the current on-chain program hash.
///   2. Look it up in `verified_hashes` filtered by the trust set
///      `{program upgrade authority} ∪ SIGNER_KEYS ∪ {DEFAULT_SIGNER}`.
///   3. Render each row in the legacy `VerificationResponseWithSigner` shape
///      so existing consumers (Solana Explorer, CLI) are unaffected.
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

    let (program_authority, is_frozen, is_closed) = match get_program_authority(&address).await {
        Ok((authority, frozen, closed)) => (authority, frozen, closed),
        Err(_) => (None, false, false),
    };

    let on_chain_hash = match get_on_chain_hash(&address).await {
        Ok(hash) => hash,
        Err(_) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::Success(SuccessResponse::StatusAll(vec![]))),
            );
        }
    };

    let trust_set = trust_set_for(program_authority.as_deref());
    match db
        .get_verified_hashes_trusted(&on_chain_hash, &trust_set)
        .await
    {
        Ok(rows) => {
            let claims: Vec<VerificationResponseWithSigner> = rows
                .into_iter()
                .map(|row| {
                    let commit = row.commit_hash.unwrap_or_default();
                    let repo_url = if commit.is_empty() {
                        row.repository
                    } else {
                        format!("{}/tree/{}", row.repository.trim_end_matches('/'), commit)
                    };
                    VerificationResponseWithSigner {
                        signer: row.signer,
                        verification_response: VerificationResponse {
                            is_verified: true,
                            on_chain_hash: on_chain_hash.clone(),
                            executable_hash: row.executable_hash,
                            repo_url,
                            commit,
                            last_verified_at: Some(row.verified_at),
                            is_frozen,
                            is_closed,
                        },
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(ApiResponse::Success(SuccessResponse::StatusAll(claims))),
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
