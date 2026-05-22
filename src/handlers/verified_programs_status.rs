//! `GET /verified-programs-status` — verification status for every verified program.

use crate::{
    db::{BuildRow, Db},
    handlers::status_message,
    onchain::build_repo_url,
    response::{Status, VerifiedProgramStatusResponse, VerifiedProgramsStatusListResponse},
};
use axum::{extract::State, http::StatusCode, Json};

pub async fn all(
    State(db): State<Db>,
) -> (StatusCode, Json<VerifiedProgramsStatusListResponse>) {
    match db.currently_verified_builds().await {
        Ok(builds) => {
            // Every row here passed the join filter, so is_verified is always
            // true and on_chain_hash == executable_hash.
            let data = builds.into_iter().map(status_row).collect();
            (
                StatusCode::OK,
                Json(VerifiedProgramsStatusListResponse {
                    status: Status::Success,
                    data: Some(data),
                    error: None,
                }),
            )
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(VerifiedProgramsStatusListResponse {
                status: Status::Error,
                data: None,
                error: Some("An unexpected database error occurred.".into()),
            }),
        ),
    }
}

fn status_row(b: BuildRow) -> VerifiedProgramStatusResponse {
    let hash = b.executable_hash.unwrap_or_default();
    VerifiedProgramStatusResponse {
        program_id: b.program_id,
        is_verified: true,
        message: status_message(true),
        on_chain_hash: hash.clone(),
        executable_hash: hash,
        last_verified_at: b.completed_at.map(|t| t.naive_utc()),
        repo_url: build_repo_url(&b.repository, b.commit_hash.as_deref()),
        commit: b.commit_hash.unwrap_or_default(),
    }
}
