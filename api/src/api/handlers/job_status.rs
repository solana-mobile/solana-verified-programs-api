use crate::db::{
    models::{JobStatus, JobVerificationResponse, SolanaProgramBuild, SolanaProgramBuildParams},
    DbClient,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::{error, info};

fn build_to_params(b: &SolanaProgramBuild) -> SolanaProgramBuildParams {
    SolanaProgramBuildParams {
        repository: b.repository.clone(),
        program_id: b.program_id.clone(),
        commit_hash: b.commit_hash.clone(),
        lib_name: b.lib_name.clone(),
        bpf_flag: Some(b.bpf_flag),
        base_image: b.base_docker_image.clone(),
        mount_path: b.mount_path.clone(),
        cargo_args: b.cargo_args.clone(),
        arch: b.arch.clone(),
        webhook_url: None,
    }
}

/// Handler for retrieving the status of a verification job
///
/// # Endpoint: GET /job/:job_id
///
/// # Arguments
/// * `db` - Database client from application state
/// * `job_id` - Unique identifier for the verification job
///
/// # Returns
/// * `(StatusCode, Json<JobVerificationResponse>)` - HTTP status and job details
pub(crate) async fn get_job_status(
    State(db): State<DbClient>,
    Path(job_id): Path<String>,
) -> (StatusCode, Json<JobVerificationResponse>) {
    info!("Checking status for job: {}", job_id);

    match db.get_job(&job_id).await {
        Ok(job) => {
            let status: JobStatus = job.status.clone().into();
            match status {
                JobStatus::Completed => {
                    info!("Job {} completed, fetching verification details", job_id);
                    let executable_hash = db
                        .find_hash_for_build_params(&build_to_params(&job))
                        .await
                        .ok()
                        .flatten()
                        .map(|h| h.executable_hash)
                        .unwrap_or_default();
                    let repo_url = job.commit_hash.clone().map_or(job.repository.clone(), |hash| {
                        format!("{}/tree/{}", job.repository.trim_end_matches('/'), hash)
                    });
                    (
                        StatusCode::OK,
                        Json(JobVerificationResponse {
                            status: JobStatus::Completed.into(),
                            message: "Job completed".to_string(),
                            on_chain_hash: String::new(), // use /status/:address for the live answer
                            executable_hash,
                            repo_url,
                        }),
                    )
                }
                JobStatus::Failed => {
                    info!("Job {} failed", job_id);
                    (
                        StatusCode::OK,
                        Json(JobVerificationResponse {
                            status: JobStatus::Failed.into(),
                            message: "Verification failed".to_string(),
                            on_chain_hash: String::new(),
                            executable_hash: String::new(),
                            repo_url: String::new(),
                        }),
                    )
                }
                JobStatus::InProgress => {
                    info!("Job {} is still in progress", job_id);
                    (
                        StatusCode::OK,
                        Json(JobVerificationResponse {
                            status: JobStatus::InProgress.into(),
                            message: "Please wait, the verification is in progress".to_string(),
                            on_chain_hash: String::new(),
                            executable_hash: String::new(),
                            repo_url: String::new(),
                        }),
                    )
                }
                JobStatus::Unused => {
                    info!("Job {} marked as unused", job_id);
                    (
                        StatusCode::OK,
                        Json(JobVerificationResponse {
                            status: JobStatus::Failed.into(),
                            message: "These params were not used. There might be a PDA associated with this program ID.".to_string(),
                            on_chain_hash: String::new(),
                            executable_hash: String::new(),
                            repo_url: String::new(),
                        }),
                    )
                }
            }
        }
        Err(err) => {
            error!("Failed to get job status from database: {}", err);
            (
                StatusCode::OK,
                create_error_response("Unexpected error while getting Data from DB"),
            )
        }
    }
}

/// Creates a standard error response
fn create_error_response(message: &str) -> Json<JobVerificationResponse> {
    Json(JobVerificationResponse {
        status: "unknown".to_string(),
        message: message.to_string(),
        on_chain_hash: String::new(),
        executable_hash: String::new(),
        repo_url: String::new(),
    })
}
