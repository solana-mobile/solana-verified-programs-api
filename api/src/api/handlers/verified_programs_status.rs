use crate::db::{
    models::{Status, VerifiedProgramStatusResponse, VerifiedProgramsStatusListResponse},
    DbClient,
};
use crate::services::get_on_chain_hash;
use crate::services::onchain::{get_program_authority, program_metadata_retriever::SIGNER_KEYS};
use crate::db::models::DEFAULT_SIGNER;
use axum::{extract::State, http::StatusCode, Json};
use futures::stream::{self, StreamExt};
use std::sync::Arc;
use tracing::{error, info};

const STATUS_ALL_CONCURRENCY: usize = 20;

/// `GET /verified-programs-status` — per-program status for every program
/// with a verified build in the directory.
///
/// For each program: RPC-fetch the current on-chain hash, look it up in
/// the directory with the trust set, and return a status row. N+1 RPC
/// pattern with bounded concurrency.
pub(crate) async fn get_verified_programs_status(
    State(db): State<DbClient>,
) -> (StatusCode, Json<VerifiedProgramsStatusListResponse>) {
    info!("Fetching status for all verified programs");

    let program_ids = match db.all_verified_program_ids().await {
        Ok(ids) => ids,
        Err(err) => {
            error!("Failed to fetch verified program ids: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(VerifiedProgramsStatusListResponse {
                    status: Status::Error,
                    data: None,
                    error: Some("An unexpected database error occurred.".to_string()),
                }),
            );
        }
    };

    let semaphore = Arc::new(tokio::sync::Semaphore::new(STATUS_ALL_CONCURRENCY));
    let results: Vec<VerifiedProgramStatusResponse> = stream::iter(program_ids.into_iter())
        .map(|program_id| {
            let db = db.clone();
            let semaphore = Arc::clone(&semaphore);
            async move {
                let _permit = semaphore.acquire().await.ok()?;
                program_status_for(&db, &program_id).await
            }
        })
        .buffer_unordered(STATUS_ALL_CONCURRENCY)
        .filter_map(|res| async move { res })
        .collect()
        .await;

    (
        StatusCode::OK,
        Json(VerifiedProgramsStatusListResponse {
            status: Status::Success,
            data: Some(results),
            error: None,
        }),
    )
}

async fn program_status_for(
    db: &DbClient,
    program_id: &str,
) -> Option<VerifiedProgramStatusResponse> {
    let (program_authority, _is_frozen, is_closed) = get_program_authority(program_id)
        .await
        .unwrap_or((None, false, false));
    if is_closed {
        return None;
    }

    let on_chain_hash = get_on_chain_hash(program_id).await.ok()?;

    let trust_set = {
        let mut out = Vec::with_capacity(2 + SIGNER_KEYS.len());
        if let Some(a) = program_authority.as_deref() {
            out.push(a.to_string());
        }
        out.extend(SIGNER_KEYS.iter().map(|k| k.to_string()));
        out.push(DEFAULT_SIGNER.to_string());
        out
    };

    let row = db
        .get_verified_hashes_trusted(&on_chain_hash, &trust_set)
        .await
        .ok()?
        .into_iter()
        .next()?;

    let commit = row.commit_hash.unwrap_or_default();
    let repo_url = if commit.is_empty() {
        row.repository
    } else {
        format!("{}/tree/{}", row.repository.trim_end_matches('/'), commit)
    };

    Some(VerifiedProgramStatusResponse {
        program_id: program_id.to_string(),
        is_verified: true,
        message: "On chain program verified".to_string(),
        on_chain_hash: on_chain_hash.clone(),
        executable_hash: on_chain_hash,
        last_verified_at: Some(row.verified_at),
        repo_url,
        commit,
    })
}
