use crate::schema::{build_logs, solana_program_builds, verified_hashes};
use chrono::{NaiveDateTime, Utc};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use solana_sdk_ids::system_program;

use super::SolanaProgramBuildParams;

pub(crate) const DEFAULT_SIGNER: Pubkey = system_program::id();

/// Represents a Solana program build in the database
#[derive(
    Clone,
    Debug,
    Serialize,
    Deserialize,
    Insertable,
    Identifiable,
    Queryable,
    AsChangeset,
    Selectable,
    QueryableByName,
    Default,
)]
#[diesel(table_name = solana_program_builds, primary_key(id))]
pub struct SolanaProgramBuild {
    pub id: String,
    pub repository: String,
    pub commit_hash: Option<String>,
    pub program_id: String,
    pub lib_name: Option<String>,
    pub base_docker_image: Option<String>,
    pub mount_path: Option<String>,
    pub cargo_args: Option<Vec<String>>,
    pub bpf_flag: bool,
    pub created_at: NaiveDateTime,
    pub status: String,
    pub signer: Option<String>,
    pub arch: Option<String>,
}

impl<'a> From<&'a SolanaProgramBuildParams> for SolanaProgramBuild {
    fn from(params: &'a SolanaProgramBuildParams) -> Self {
        SolanaProgramBuild {
            id: uuid::Uuid::new_v4().to_string(),
            repository: params.repository.clone(),
            commit_hash: params.commit_hash.clone(),
            program_id: params.program_id.clone(),
            lib_name: params.lib_name.clone(),
            bpf_flag: params.bpf_flag.unwrap_or(false),
            created_at: Utc::now().naive_utc(),
            base_docker_image: params.base_image.clone(),
            mount_path: params.mount_path.clone(),
            cargo_args: params.cargo_args.clone(),
            status: JobStatus::InProgress.into(),
            signer: Some(DEFAULT_SIGNER.to_string()),
            arch: params.arch.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum JobStatus {
    #[serde(rename = "in_progress")]
    InProgress,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "un-used")]
    Unused,
}

impl From<JobStatus> for String {
    fn from(status: JobStatus) -> Self {
        match status {
            JobStatus::InProgress => "in_progress".to_string(),
            JobStatus::Completed => "completed".to_string(),
            JobStatus::Failed => "failed".to_string(),
            JobStatus::Unused => "un-used".to_string(),
        }
    }
}

impl From<String> for JobStatus {
    fn from(status: String) -> Self {
        match status.as_str() {
            "in_progress" => JobStatus::InProgress,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            "un-used" => JobStatus::Unused,
            _ => panic!("Invalid job status"),
        }
    }
}

/// Content-addressed verified-build claim. A row asserts that signer `signer`
/// claims `(repository, commit_hash, build_args)` deterministically produces
/// `executable_hash`. Multiple signers may claim the same hash.
#[derive(
    Clone, Debug, Serialize, Deserialize, Insertable, Identifiable, Queryable, AsChangeset,
    Selectable, QueryableByName,
)]
#[diesel(table_name = verified_hashes, primary_key(executable_hash, signer))]
pub struct VerifiedHash {
    pub executable_hash: String,
    pub signer: String,
    pub repository: String,
    pub commit_hash: Option<String>,
    pub lib_name: Option<String>,
    pub base_docker_image: Option<String>,
    pub mount_path: Option<String>,
    pub cargo_args: Option<Vec<String>>,
    pub bpf_flag: bool,
    pub arch: Option<String>,
    pub verified_at: NaiveDateTime,
}

impl VerifiedHash {
    /// Build a `VerifiedHash` row from a completed program build and its produced hash.
    pub fn from_build(
        build: &SolanaProgramBuild,
        executable_hash: impl Into<String>,
        signer: impl Into<String>,
    ) -> Self {
        Self {
            executable_hash: executable_hash.into(),
            signer: signer.into(),
            repository: build.repository.clone(),
            commit_hash: build.commit_hash.clone(),
            lib_name: build.lib_name.clone(),
            base_docker_image: build.base_docker_image.clone(),
            mount_path: build.mount_path.clone(),
            cargo_args: build.cargo_args.clone(),
            bpf_flag: build.bpf_flag,
            arch: build.arch.clone(),
            verified_at: Utc::now().naive_utc(),
        }
    }
}

/// Build logs row.
#[derive(Clone, Debug, Serialize, Deserialize, Insertable, Queryable, AsChangeset)]
#[diesel(table_name = build_logs, primary_key(id))]
pub struct BuildLogs {
    pub id: String,
    pub program_address: String,
    pub file_name: String,
    pub created_at: NaiveDateTime,
}
