//! Wire response shapes. Pin the pre-rewrite JSON output byte for byte;
//! handlers convert from domain types ([`crate::db::BuildRow`] etc.) at the
//! boundary.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResponse {
    pub is_verified: bool,
    pub on_chain_hash: String,
    pub executable_hash: String,
    pub repo_url: String,
    pub commit: String,
    pub last_verified_at: Option<NaiveDateTime>,
    pub is_frozen: bool,
    pub is_closed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerificationResponseWithSigner {
    pub signer: String,
    #[serde(flatten)]
    pub verification_response: VerificationResponse,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub is_verified: bool,
    pub message: String,
    pub on_chain_hash: String,
    pub executable_hash: String,
    pub repo_url: String,
    pub commit: String,
    pub last_verified_at: Option<NaiveDateTime>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtendedStatusResponse {
    #[serde(flatten)]
    pub status: StatusResponse,
    pub is_frozen: bool,
    pub is_closed: bool,
}

/// `request_id` is the build UUID the caller polls `/job/:id` with.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub status: String,
    pub request_id: String,
    pub message: String,
}

/// Hash/url fields are empty strings (not null) for in-progress or failed
/// jobs — preserved from the legacy shape.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobVerificationResponse {
    pub status: String,
    pub message: String,
    pub on_chain_hash: String,
    pub executable_hash: String,
    pub repo_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaginationMeta {
    pub total: i64,
    pub page: i64,
    pub total_pages: i64,
    pub items_per_page: i64,
    pub has_next_page: bool,
    pub has_prev_page: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifiedProgramListResponse {
    pub meta: PaginationMeta,
    pub verified_programs: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifiedProgramStatusResponse {
    pub program_id: String,
    pub is_verified: bool,
    pub message: String,
    pub on_chain_hash: String,
    pub executable_hash: String,
    pub last_verified_at: Option<NaiveDateTime>,
    pub repo_url: String,
    pub commit: String,
}

/// Serialized as `"success"`/`"error"`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Success,
    Error,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifiedProgramsStatusListResponse {
    pub status: Status,
    pub data: Option<Vec<VerifiedProgramStatusResponse>>,
    pub error: Option<String>,
}

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

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveHashResponse {
    pub executable_hash: String,
    pub builds: Vec<ResolveHashEntry>,
}

/// `is_currently_on_chain` is true when this build's hash matches its
/// program's cached `on_chain_hash`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveHashEntry {
    pub build_id: String,
    pub program_id: String,
    pub signer: Option<String>,
    pub repository: String,
    pub commit: Option<String>,
    pub completed_at: Option<NaiveDateTime>,
    pub is_currently_on_chain: bool,
}

/// `status` is one of `Active`/`Inactive`/`unknown`; field name preserved
/// from the legacy `BackgroundJobManager` shape.
#[derive(Debug, Serialize, Deserialize)]
pub struct BackgroundJobHealth {
    pub status: String,
    pub last_program_check: Option<NaiveDateTime>,
    pub message: String,
}
