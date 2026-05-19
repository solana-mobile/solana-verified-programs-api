use super::verify_helpers::{
    create_and_insert_build, create_internal_error, setup_verification, validate_program_id,
    validate_repository_url,
};
use crate::{
    db::{
        models::{ApiResponse, SolanaProgramBuild, SolanaProgramBuildParams, StatusResponse},
        DbClient,
    },
    services::{
        build_repository_url, get_on_chain_hash,
        verification::{check_and_handle_duplicates, process_verification_request},
    },
};
use axum::{extract::State, http::StatusCode, Json};
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
    State(db): State<DbClient>,
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
    db: DbClient,
    payload: SolanaProgramBuildParams,
    signer: String,
) -> (StatusCode, Json<ApiResponse>) {
    // Content-addressed fast path: if the directory already has this
    // `(repository, commit, build_args)`, skip the slow build and just
    // compare the cached hash against the current on-chain bytes.
    if let Ok(Some(cached)) = db.find_hash_for_build_params(&payload).await {
        info!(
            "Directory cache hit for {}: comparing cached hash {} against on-chain",
            payload.program_id, cached.executable_hash
        );
        let on_chain_hash = get_on_chain_hash(&payload.program_id).await.unwrap_or_default();
        let is_verified = !on_chain_hash.is_empty() && on_chain_hash == cached.executable_hash;
        let mut build = SolanaProgramBuild::from(&payload);
        build.signer = Some(signer.clone());
        return (
            StatusCode::OK,
            Json(
                StatusResponse {
                    is_verified,
                    message: if is_verified {
                        "On chain program verified"
                    } else {
                        "On chain program not verified"
                    }
                    .to_string(),
                    on_chain_hash,
                    executable_hash: cached.executable_hash,
                    last_verified_at: Some(cached.verified_at),
                    repo_url: build_repository_url(&build),
                    commit: payload.commit_hash.clone().unwrap_or_default(),
                }
                .into(),
            ),
        );
    }

    // Check for existing verification
    if let Some(response) = check_and_handle_duplicates(&payload, signer.clone(), &db).await {
        return (StatusCode::OK, Json(response.into()));
    }

    // Create and insert build parameters
    let uuid = match create_and_insert_build(&db, &payload, &signer).await {
        Ok(uuid) => uuid,
        Err(error_response) => return error_response,
    };

    // Process verification synchronously
    match process_verification_request(payload.clone(), &uuid, &db).await {
        Ok(res) => {
            info!(
                "Verification completed for program: {} (verified: {})",
                payload.program_id, res.is_verified
            );

            (
                StatusCode::OK,
                Json(
                    StatusResponse {
                        is_verified: res.is_verified,
                        message: if res.is_verified {
                            "On chain program verified"
                        } else {
                            "On chain program not verified"
                        }
                        .to_string(),
                        on_chain_hash: res.on_chain_hash,
                        executable_hash: res.executable_hash,
                        last_verified_at: Some(res.verified_at),
                        repo_url: {
                            let mut build = SolanaProgramBuild::from(&payload);
                            build.signer = Some(signer.clone());
                            build_repository_url(&build)
                        },
                        commit: payload.commit_hash.clone().unwrap_or_default(),
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
