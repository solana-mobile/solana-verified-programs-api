//! `GET /job/:job_id` — async build status.

use crate::{
    db::Db,
    error::{ApiError, Result},
    onchain::build_repo_url,
    response::JobVerificationResponse,
};
use axum::{
    extract::{Path, State},
    Json,
};
use std::str::FromStr;
use uuid::Uuid;

pub async fn get_job_status(
    State(db): State<Db>,
    Path(job_id): Path<String>,
) -> Result<Json<JobVerificationResponse>> {
    let id = Uuid::from_str(&job_id)
        .map_err(|_| ApiError::BadRequest("invalid job id (expected UUID)".into()))?;
    let build = db
        .get_build(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("job {job_id} not found")))?;
    let resp = match build.status.as_str() {
        "completed" => JobVerificationResponse {
            status: "completed".into(),
            message: "Job completed".into(),
            on_chain_hash: db
                .get_program_state(&build.program_id)
                .await
                .ok()
                .flatten()
                .and_then(|s| s.on_chain_hash)
                .unwrap_or_default(),
            executable_hash: build.executable_hash.unwrap_or_default(),
            repo_url: build_repo_url(&build.repository, build.commit_hash.as_deref()),
        },
        "failed" => JobVerificationResponse {
            status: "failed".into(),
            message: build
                .error_message
                .unwrap_or_else(|| "Verification failed".into()),
            on_chain_hash: String::new(),
            executable_hash: String::new(),
            repo_url: String::new(),
        },
        _ => JobVerificationResponse {
            status: "in_progress".into(),
            message: "Please wait, the verification is in progress".into(),
            on_chain_hash: String::new(),
            executable_hash: String::new(),
            repo_url: String::new(),
        },
    };
    Ok(Json(resp))
}
