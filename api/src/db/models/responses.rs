use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use super::JobStatus;

/// Payload posted to webhook when verification completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationWebhookPayload {
    pub request_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_chain_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<NaiveDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// General API response status.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Success,
    Error,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub status: Status,
    pub error: String,
}

/// Response body for `GET /status/:address`.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub is_verified: bool,
    pub message: String,
    pub on_chain_hash: String,
    pub executable_hash: String,
    pub repo_url: String,
    pub commit: String,
    pub last_verified_at: Option<NaiveDateTime>,
    /// Signer whose directory row satisfied the trust filter, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
}

/// Extends `StatusResponse` with the program-authority flags.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExtendedStatusResponse {
    #[serde(flatten)]
    pub status: StatusResponse,
    pub is_frozen: bool,
    pub is_closed: bool,
}

/// Response for `POST /verify*`.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub status: JobStatus,
    pub request_id: String,
    pub message: String,
    /// Executable hash, populated when known immediately (directory cache hit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_hash: Option<String>,
}

/// Successful API response wrapper (untagged so it serializes as the inner shape).
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SuccessResponse {
    Status(StatusResponse),
    Verify(VerifyResponse),
}

impl From<StatusResponse> for SuccessResponse {
    fn from(value: StatusResponse) -> Self {
        Self::Status(value)
    }
}

/// Either-success-or-error API envelope.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApiResponse {
    Success(SuccessResponse),
    Error(ErrorResponse),
    /// List of content-addressed claims (used by `/resolve-hash/:hash` and `/status-all/:address`).
    ResolveHashList(Vec<ResolveHashResponse>),
}

impl From<StatusResponse> for ApiResponse {
    fn from(value: StatusResponse) -> Self {
        Self::Success(SuccessResponse::Status(value))
    }
}

impl From<VerifyResponse> for ApiResponse {
    fn from(value: VerifyResponse) -> Self {
        Self::Success(SuccessResponse::Verify(value))
    }
}

impl From<ErrorResponse> for ApiResponse {
    fn from(value: ErrorResponse) -> Self {
        Self::Error(value)
    }
}

/// Response for `GET /job/:job_id`.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobVerificationResponse {
    pub status: String,
    pub message: String,
    pub on_chain_hash: String,
    pub executable_hash: String,
    pub repo_url: String,
}

/// Response for `GET /resolve-hash/:hash`: one signer's claim about a hash.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveHashResponse {
    pub executable_hash: String,
    pub signer: String,
    pub repository: String,
    pub commit: Option<String>,
    pub build_args: BuildArgs,
    pub verified_at: NaiveDateTime,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct BuildArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cargo_args: Option<Vec<String>>,
    pub bpf_flag: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}
