//! Shared verification helpers and utilities
//! Contains common logic used across different verification endpoints

use crate::{
    db::{Db, NewBuild},
    error::{ApiError, Result},
    onchain, validation,
};
use solana_pubkey::Pubkey;
use tracing::error;
use uuid::Uuid;

/// Validates a Solana program ID, returning a `Pubkey` on success.
pub fn validate_program_id(program_id: &str) -> Result<Pubkey> {
    validation::validate_pubkey(program_id).map_err(ApiError::BadRequest)
}

/// Same as `validate_program_id` — distinct name keeps the call site
/// readable.
pub fn validate_signer(signer: &str) -> Result<Pubkey> {
    validation::validate_pubkey(signer).map_err(ApiError::BadRequest)
}

/// Validates a `https://` URL (or `http://` for loopback hosts only).
pub fn validate_repository_url(repository: &str) -> Result<()> {
    validation::validate_http_url(repository).map_err(ApiError::BadRequest)
}

/// Validates an optional webhook URL; `None` is fine.
pub fn validate_webhook_url(webhook_url: &Option<String>) -> Result<()> {
    if let Some(url) = webhook_url.as_deref() {
        validation::validate_http_url(url).map_err(ApiError::BadRequest)?;
    }
    Ok(())
}

/// Common setup logic for verification endpoints
///
/// Handles:
/// - Refreshing `program_state` from chain (authority, frozen/closed flags)
/// - Fetching verification parameters from the on-chain Otter Verify PDA
///
/// Returns the `NewBuild` row the caller should insert, plus the signer
/// whose PDA claim was used.
pub async fn setup_verification(
    db: &Db,
    program_id: &Pubkey,
    specific_signer: Option<Pubkey>,
) -> Result<(NewBuild, String)> {
    let state = onchain::get_program_state(program_id)
        .await
        .unwrap_or(onchain::ProgramOnchainState {
            authority: None,
            is_frozen: false,
            is_closed: false,
            executable_hash: None,
        });

    let (params, signer) = onchain::get_otter_verify_params(
        program_id,
        specific_signer,
        state.authority.as_deref(),
    )
    .await
    .map_err(|e| {
        error!(
            "Unable to find on-chain PDA for program {}: {:?}",
            program_id, e
        );
        ApiError::NotFound("Otter Verify PDA not found".into())
    })?;

    if let Err(e) = db.upsert_program_state(&params.address.to_string(), &state).await {
        error!("Failed to update program state: {:?}", e);
    }

    let build = NewBuild {
        repository: params.git_url.clone(),
        commit_hash: Some(params.commit.clone()),
        program_id: params.address.to_string(),
        lib_name: params.library_name(),
        base_docker_image: params.base_image(),
        mount_path: params.mount_path(),
        cargo_args: params.cargo_args(),
        bpf_flag: params.bpf(),
        arch: params.arch(),
        signer: Some(signer.clone()),
    };
    Ok((build, signer))
}

/// Inserts a new `in_progress` build row and returns its UUID.
pub async fn create_and_insert_build(db: &Db, params: &NewBuild) -> Result<Uuid> {
    db.insert_build(params).await.inspect_err(|e| {
        error!("Error inserting build parameters: {:?}", e);
    })
}
