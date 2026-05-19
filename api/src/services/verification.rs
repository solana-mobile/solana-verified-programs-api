use crate::{
    db::models::{
        JobStatus, SolanaProgramBuild, SolanaProgramBuildParams, VerificationWebhookPayload,
        VerifiedHash,
    },
    db::DbClient,
    errors::ApiError,
    services::misc::extract_hash_with_prefix,
    Result, CONFIG,
};
use chrono::NaiveDateTime;
use std::process::Stdio;
use std::time::Duration;
use tokio::{io::AsyncWriteExt, process::Command, time::sleep};
use tracing::{error, info};
use uuid::Uuid;

const MAX_WEBHOOK_RETRIES: u32 = 3;
const WEBHOOK_RETRY_DELAY_MS: u64 = 2000;

/// Outcome of a single build run. The directory entry (`verified_hashes`) is
/// the source of truth; this struct just carries the bytes needed by the
/// synchronous response and by webhook delivery.
#[derive(Debug, Clone)]
pub struct BuildResult {
    pub program_id: String,
    pub executable_hash: String,
    pub on_chain_hash: String,
    pub is_verified: bool,
    pub verified_at: NaiveDateTime,
}

/// Run a build, update job status, and populate the content-addressed directory.
pub async fn process_verification_request(
    payload: SolanaProgramBuildParams,
    build_id: &str,
    db: &DbClient,
) -> Result<BuildResult> {
    let random_file_id = Uuid::new_v4().to_string();
    let program_id = payload.program_id.clone();
    let payload_for_directory = payload.clone();

    match execute_verification(payload, build_id, &random_file_id).await {
        Ok(res) => {
            if let Err(e) = db.update_build_status(build_id, JobStatus::Completed).await {
                error!("Failed to update build status to completed: {:?}", e);
            }
            if !res.executable_hash.is_empty() {
                let build = SolanaProgramBuild::from(&payload_for_directory);
                let entry = VerifiedHash::from_build(&build, res.executable_hash.clone());
                if let Err(e) = db.insert_or_update_verified_hash(&entry).await {
                    error!("Failed to populate verified_hashes directory: {:?}", e);
                }
            }
            Ok(res)
        }
        Err(err) => {
            if let Err(e) = db.update_build_status(build_id, JobStatus::Failed).await {
                error!("Failed to update build status to failed: {:?}", e);
            }
            if let Err(e) = db
                .insert_logs_info(&random_file_id, &program_id, build_id)
                .await
            {
                error!("Failed to insert logs info: {:?}", e);
            }
            error!("Build verification failed: {:?}", err);
            Err(err)
        }
    }
}

/// Invoke `solana-verify verify-from-repo` and parse out the build & on-chain hashes.
pub async fn execute_verification(
    payload: SolanaProgramBuildParams,
    _build_id: &str,
    random_file_id: &str,
) -> Result<BuildResult> {
    info!(
        "Starting build verification for program: {}",
        payload.program_id
    );

    let mut cmd = build_verify_command(&payload);

    let mut child = cmd.spawn().map_err(|e| {
        error!("Failed to spawn solana-verify command: {}", e);
        ApiError::Build("Failed to start verification process".to_string())
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"n\n").await.map_err(|e| {
            error!("Failed to write to stdin: {}", e);
            ApiError::Build("Failed to communicate with verification process".to_string())
        })?;
    }

    let output = child.wait_with_output().await.map_err(|e| {
        error!("Failed to get command output: {}", e);
        ApiError::Build("Failed to complete verification process".to_string())
    })?;

    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr).unwrap_or_default();
        if let Err(e) =
            crate::services::logging::write_logs(&stderr, &stdout, random_file_id).await
        {
            error!("Failed to write logs: {:?}", e);
        }
        return Err(ApiError::Build(stdout));
    }

    let on_chain_hash =
        extract_hash_with_prefix(&stdout, "On-chain Program Hash:").unwrap_or_default();
    let executable_hash =
        extract_hash_with_prefix(&stdout, "Executable Program Hash from repo:").unwrap_or_default();

    info!(
        "Verification complete — program: {}, build hash: {}, on-chain hash: {}",
        payload.program_id, executable_hash, on_chain_hash
    );

    Ok(BuildResult {
        program_id: payload.program_id.clone(),
        is_verified: !executable_hash.is_empty() && executable_hash == on_chain_hash,
        executable_hash,
        on_chain_hash,
        verified_at: chrono::Utc::now().naive_utc(),
    })
}

fn build_verify_command(payload: &SolanaProgramBuildParams) -> Command {
    let mut cmd = Command::new("solana-verify");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg("verify-from-repo")
        .arg("--url")
        .arg(&CONFIG.rpc_url)
        .arg("--program-id")
        .arg(&payload.program_id)
        .arg(&payload.repository);

    if let Some(ref commit) = payload.commit_hash {
        cmd.arg("--commit-hash").arg(commit);
    }
    if let Some(ref lib_name) = payload.lib_name {
        cmd.arg("--library-name").arg(lib_name);
    }
    if let Some(ref base_image) = payload.base_image {
        cmd.arg("--base-image").arg(base_image);
    }
    if let Some(ref mount_path) = payload.mount_path {
        cmd.arg("--mount-path").arg(mount_path);
    }
    if payload.bpf_flag.unwrap_or(false) {
        cmd.arg("--bpf");
    }
    if let Some(ref arch) = payload.arch {
        cmd.arg("--arch").arg(arch);
    }
    if let Some(ref cargo_args) = payload.cargo_args {
        cmd.arg("--").args(cargo_args);
    }
    cmd
}

pub async fn notify_webhook(
    webhook_url: String,
    result: std::result::Result<BuildResult, ApiError>,
    request_id: String,
) {
    let payload = match &result {
        Ok(v) => VerificationWebhookPayload {
            request_id: request_id.clone(),
            status: "completed".to_string(),
            is_verified: Some(v.is_verified),
            program_id: Some(v.program_id.clone()),
            on_chain_hash: Some(v.on_chain_hash.clone()),
            executable_hash: Some(v.executable_hash.clone()),
            verified_at: Some(v.verified_at),
            error: None,
        },
        Err(e) => VerificationWebhookPayload {
            request_id,
            status: "failed".to_string(),
            is_verified: None,
            program_id: None,
            on_chain_hash: None,
            executable_hash: None,
            verified_at: None,
            error: Some(e.to_string()),
        },
    };
    if let Err(e) = post_webhook(&webhook_url, &payload).await {
        error!("Webhook failed to post payload to {}: {:?}", webhook_url, e);
    }
}

async fn post_webhook(
    url: &str,
    payload: &VerificationWebhookPayload,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    let mut last_error = None;
    for attempt in 0..MAX_WEBHOOK_RETRIES {
        match client.post(url).json(payload).send().await {
            Ok(res) => match res.error_for_status() {
                Ok(_) => return Ok(()),
                Err(e) => last_error = Some(e.into()),
            },
            Err(e) => last_error = Some(e.into()),
        }
        if attempt < MAX_WEBHOOK_RETRIES - 1 {
            sleep(Duration::from_millis(WEBHOOK_RETRY_DELAY_MS)).await;
        }
    }
    Err(last_error
        .unwrap_or_else(|| Box::from(std::io::Error::other("webhook post failed after retries"))))
}
