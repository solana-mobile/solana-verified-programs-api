use crate::db::models::{
    ApiResponse, ErrorResponse, ExtendedStatusResponse, ResolveHashResponse, Status,
    StatusResponse, VerificationStatusParams,
};
use crate::db::DbClient;
use crate::services::get_on_chain_hash;
use crate::services::onchain::{get_program_authority, program_metadata_retriever::SIGNER_KEYS};
use crate::validation::{self, validate_executable_hash};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::{error, info};

/// `GET /status/:address` — thin wrapper over the content-addressed directory,
/// filtered by trust.
///
///   1. Fetch the current on-chain program hash via RPC.
///   2. Build the trust set: `{program_upgrade_authority} ∪ SIGNER_KEYS`.
///   3. Look up directory rows for `(on_chain_hash, signer ∈ trust_set)`.
///   4. Return verified iff any trusted signer has claimed this hash, with
///      that signer surfaced in the response.
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
                        signer: Some(entry.signer),
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

/// `GET /status-all/:id` — list every claim about a build.
///
/// `:id` is polymorphic by shape:
///
/// - **64-character hex** (an executable hash): returns every signer's claim
///   about that hash, trust filter not applied (the caller chose to look up
///   bytes, so they decide whose claim to weigh).
/// - **base58 pubkey** (a program id): fetches the program's current on-chain
///   hash and returns every *trusted* signer's claim for it (trust set =
///   `{upgrade authority} ∪ SIGNER_KEYS`).
pub(crate) async fn get_verification_status_all(
    State(db): State<DbClient>,
    Path(VerificationStatusParams { address: id }): Path<VerificationStatusParams>,
) -> (StatusCode, Json<ApiResponse>) {
    // Hash form: hex 64 chars.
    if validate_executable_hash(&id).is_ok() {
        info!("status-all: hash lookup for {}", id);
        return match db.get_verified_hashes_by_hash(&id).await {
            Ok(rows) => {
                let claims: Vec<ResolveHashResponse> = rows.into_iter().map(Into::into).collect();
                (StatusCode::OK, Json(ApiResponse::ResolveHashList(claims)))
            }
            Err(e) => {
                error!("status-all hash lookup failed: {:?}", e);
                (StatusCode::OK, Json(ApiResponse::ResolveHashList(vec![])))
            }
        };
    }

    // Pubkey form: program id, trust-filtered against the on-chain hash.
    if let Err(e) = validation::validate_pubkey(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                ErrorResponse {
                    status: Status::Error,
                    error: format!("Path param must be a 64-char hex hash or a base58 pubkey: {e}"),
                }
                .into(),
            ),
        );
    }

    info!("status-all: program lookup for {}", id);
    let (program_authority, _, _) = get_program_authority(&id).await.unwrap_or((None, false, false));
    let on_chain_hash = match get_on_chain_hash(&id).await {
        Ok(hash) => hash,
        Err(_) => return (StatusCode::OK, Json(ApiResponse::ResolveHashList(vec![]))),
    };

    let trust_set = trust_set_for(program_authority.as_deref());
    match db
        .get_verified_hashes_trusted(&on_chain_hash, &trust_set)
        .await
    {
        Ok(rows) => {
            let claims: Vec<ResolveHashResponse> = rows.into_iter().map(Into::into).collect();
            (StatusCode::OK, Json(ApiResponse::ResolveHashList(claims)))
        }
        Err(e) => {
            error!("status-all program lookup failed: {:?}", e);
            (StatusCode::OK, Json(ApiResponse::ResolveHashList(vec![])))
        }
    }
}

/// Trust ordering: program upgrade authority first, then whitelisted Otter
/// signers, in the order declared in `SIGNER_KEYS`.
fn trust_set_for(program_authority: Option<&str>) -> Vec<String> {
    let mut out = Vec::with_capacity(1 + SIGNER_KEYS.len());
    if let Some(a) = program_authority {
        out.push(a.to_string());
    }
    out.extend(SIGNER_KEYS.iter().map(|k| k.to_string()));
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
            signer: None,
        },
        is_frozen,
        is_closed,
    }
}
