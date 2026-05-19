use crate::db::{
    models::{JobStatus, JobVerificationResponse, SolanaProgramBuild, SolanaProgramBuildParams},
    DbClient,
};
use crate::services::build_repository_url;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::{error, info};

/// `GET /job/:job_id` — status of a verification job.
///
/// Completed jobs return the executable hash by looking the build's params
/// up in the content-addressed directory (the job table itself never stores
/// the resulting hash).
pub(crate) async fn get_job_status(
    State(db): State<DbClient>,
    Path(job_id): Path<String>,
) -> (StatusCode, Json<JobVerificationResponse>) {
    info!("Checking status for job: {}", job_id);

    let job = match db.get_job(&job_id).await {
        Ok(j) => j,
        Err(err) => {
            error!("Failed to get job: {}", err);
            return (StatusCode::OK, Json(error_response("Job lookup failed")));
        }
    };

    let status: JobStatus = job.status.clone().into();
    match status {
        JobStatus::Completed => {
            let params = params_from_build(&job);
            let executable_hash = db
                .find_hash_for_build_params(&params)
                .await
                .ok()
                .flatten()
                .map(|h| h.executable_hash)
                .unwrap_or_default();
            let repo_url = build_repository_url(&job);
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
        JobStatus::Failed => (
            StatusCode::OK,
            Json(JobVerificationResponse {
                status: JobStatus::Failed.into(),
                message: "Verification failed".to_string(),
                on_chain_hash: String::new(),
                executable_hash: String::new(),
                repo_url: String::new(),
            }),
        ),
        JobStatus::InProgress => (
            StatusCode::OK,
            Json(JobVerificationResponse {
                status: JobStatus::InProgress.into(),
                message: "Please wait, the verification is in progress".to_string(),
                on_chain_hash: String::new(),
                executable_hash: String::new(),
                repo_url: String::new(),
            }),
        ),
        JobStatus::Unused => (
            StatusCode::OK,
            Json(JobVerificationResponse {
                status: JobStatus::Failed.into(),
                message: "These params were not used. There might be a PDA associated with this program ID.".to_string(),
                on_chain_hash: String::new(),
                executable_hash: String::new(),
                repo_url: String::new(),
            }),
        ),
    }
}

fn params_from_build(build: &SolanaProgramBuild) -> SolanaProgramBuildParams {
    SolanaProgramBuildParams {
        repository: build.repository.clone(),
        program_id: build.program_id.clone(),
        commit_hash: build.commit_hash.clone(),
        lib_name: build.lib_name.clone(),
        bpf_flag: Some(build.bpf_flag),
        base_image: build.base_docker_image.clone(),
        mount_path: build.mount_path.clone(),
        cargo_args: build.cargo_args.clone(),
        arch: build.arch.clone(),
        webhook_url: None,
    }
}

fn error_response(message: &str) -> JobVerificationResponse {
    JobVerificationResponse {
        status: "unknown".to_string(),
        message: message.to_string(),
        on_chain_hash: String::new(),
        executable_hash: String::new(),
        repo_url: String::new(),
    }
}
