use crate::db::models::{ExtendedStatusResponse, StatusResponse, VerificationStatusParams};
use crate::db::DbClient;
use crate::services::get_on_chain_hash;
use crate::services::onchain::get_program_authority;
use crate::validation;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::{error, info};

/// `GET /status/:address` — thin wrapper over the content-addressed directory.
///
///   1. Fetch the current on-chain program hash via RPC.
///   2. Look that hash up in `verified_hashes`.
///   3. Return verified iff the directory has an entry for it.
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

    let (is_frozen, is_closed) = match get_program_authority(&address).await {
        Ok((_, frozen, closed)) => (frozen, closed),
        Err(_) => (false, false),
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

    match db.get_verified_hash(&on_chain_hash).await {
        Ok(Some(entry)) => {
            let commit = entry.commit_hash.clone().unwrap_or_default();
            let repo_url = if commit.is_empty() {
                entry.repository.clone()
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
        Ok(None) => (
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
