use crate::services::onchain::OtterBuildParams;
use serde::{Deserialize, Serialize};

/// Parameters for Solana program build operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SolanaProgramBuildParams {
    /// GitHub repository URL
    pub repository: String,
    /// Solana program ID
    pub program_id: String,
    /// Git commit hash
    pub commit_hash: Option<String>,
    /// Library name for the program
    pub lib_name: Option<String>,
    /// Flag to indicate BPF compilation
    pub bpf_flag: Option<bool>,
    /// Base Docker image for build
    pub base_image: Option<String>,
    /// Mount path in container
    pub mount_path: Option<String>,
    /// Additional cargo build arguments
    pub cargo_args: Option<Vec<String>>,
    /// Architecture target
    pub arch: Option<String>,
    /// Optional webhook URL to POST verification result/error when job completes
    pub webhook_url: Option<String>,
}

/// Build parameters with associated PDA signer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SolanaProgramBuildParamsWithSigner {
    /// Signer's public key
    pub signer: String,
    /// Solana program ID
    pub program_id: String,
    /// Optional webhook URL to POST verification result/error when job completes
    pub webhook_url: Option<String>,
}

impl From<OtterBuildParams> for SolanaProgramBuildParams {
    fn from(otter: OtterBuildParams) -> Self {
        SolanaProgramBuildParams {
            repository: otter.git_url.clone(),
            program_id: otter.address.to_string(),
            commit_hash: Some(otter.commit.clone()),
            lib_name: otter.get_library_name(),
            bpf_flag: Some(otter.is_bpf()),
            base_image: otter.get_base_image(),
            mount_path: otter.get_mount_path(),
            cargo_args: otter.get_cargo_args(),
            arch: otter.get_arch(),
            webhook_url: None,
        }
    }
}

/// Parameters for `POST /compute-hash`. Pure build config — no `program_id`,
/// since the content-addressed directory is keyed only by what determines the bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeHashParams {
    pub repository: String,
    pub commit_hash: Option<String>,
    pub lib_name: Option<String>,
    pub bpf_flag: Option<bool>,
    pub base_image: Option<String>,
    pub mount_path: Option<String>,
    pub cargo_args: Option<Vec<String>>,
    pub arch: Option<String>,
    /// Optional program_id to attach for the underlying `solana-verify` build job.
    /// Required by today's `solana-verify verify-from-repo` driver, but the directory
    /// entry it produces is content-addressed and is not bound to this program_id.
    pub program_id: Option<String>,
    /// Optional webhook URL to POST verification result/error when job completes.
    pub webhook_url: Option<String>,
}

impl ComputeHashParams {
    /// Promote into the legacy build params shape used by the verification driver.
    /// `program_id` is required by the driver; the caller decides what to pass.
    pub fn into_build_params(self, program_id: String) -> SolanaProgramBuildParams {
        SolanaProgramBuildParams {
            repository: self.repository,
            program_id,
            commit_hash: self.commit_hash,
            lib_name: self.lib_name,
            bpf_flag: self.bpf_flag,
            base_image: self.base_image,
            mount_path: self.mount_path,
            cargo_args: self.cargo_args,
            arch: self.arch,
            webhook_url: self.webhook_url,
        }
    }
}

/// Query params for verified programs list
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct VerifiedProgramsQuery {
    /// Optional search: valid address or HTTP/HTTPS URL to filter by program_id or repo
    pub search: Option<String>,
}

/// Parameters for verification status requests
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct VerificationStatusParams {
    /// Program address to check
    pub address: String,
}

#[derive(Clone)]
pub struct ProgramAuthorityParams {
    pub authority: Option<String>,
    pub frozen: bool,
    pub closed: bool,
}

/// Complete program authority data from database
#[derive(Debug, Clone)]
pub struct ProgramAuthorityData {
    pub authority: Option<String>,
    pub is_frozen: bool,
    pub is_closed: bool,
}
