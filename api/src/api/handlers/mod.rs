//! API request handlers for the verification service.
//! Each module corresponds to a specific API endpoint or related group of endpoints.

pub mod async_verify;
pub mod health;
pub mod job_status;
pub mod logs;
pub mod resolve_hash;
pub mod sync_verify;
pub mod verification_status;
pub mod verify_helpers;

pub(crate) use async_verify::{process_async_verification, process_async_verification_with_signer};
pub(crate) use health::health_check;
pub(crate) use job_status::get_job_status;
pub(crate) use logs::get_build_logs;
pub(crate) use resolve_hash::resolve_hash;
pub(crate) use sync_verify::process_sync_verification;
pub(crate) use verification_status::{get_verification_status, get_verification_status_all};
