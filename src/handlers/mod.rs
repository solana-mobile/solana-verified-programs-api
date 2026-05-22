pub mod async_verify;
pub mod health;
pub mod index;
pub mod job_status;
pub mod logs;
pub mod pda_worker;
pub mod resolve_hash;
pub mod sync_verify;
pub mod unverify;
pub mod verification_status;
pub mod verified_programs_list;
pub mod verified_programs_status;
pub mod verify_helpers;

use crate::CONFIG;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::Value;

/// Constant-equality check against [`crate::config::Config::auth_secret`].
pub fn is_authorized(headers: &HeaderMap) -> bool {
    headers
        .get("AUTHORIZATION")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == CONFIG.auth_secret)
}

/// Subset of Helius's parsed-transaction payload we actually look at. Extra
/// fields are ignored by serde's default behaviour.
#[derive(Debug, Deserialize)]
pub struct HeliusParsedTransaction {
    pub instructions: Vec<Instruction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instruction {
    pub accounts: Vec<String>,
    pub data: String,
    pub program_id: String,
}

pub(crate) fn parse_helius_transaction(
    payload: &[Value],
) -> std::result::Result<HeliusParsedTransaction, (StatusCode, &'static str)> {
    if payload.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Empty payload"));
    }
    serde_json::from_value(payload[0].clone())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid payload"))
}

