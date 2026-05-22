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

use crate::{
    config::CONFIG,
    db::Db,
    error::{ApiError, Result},
    onchain::snapshot_programs,
};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use tracing::info;

/// Constant-equality check against [`crate::config::Config::auth_secret`].
pub fn is_authorized(headers: &HeaderMap) -> bool {
    headers
        .get("AUTHORIZATION")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == CONFIG.auth_secret)
}

/// Human-readable verification summary used by several status responses.
pub fn status_message(is_verified: bool) -> String {
    if is_verified {
        "On chain program verified".into()
    } else {
        "On chain program not verified".into()
    }
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

pub(crate) fn parse_helius(
    payload: &[Value],
) -> std::result::Result<HeliusParsedTransaction, (StatusCode, &'static str)> {
    if payload.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Empty payload"));
    }
    serde_json::from_value(payload[0].clone())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid payload"))
}

/// Snapshot a single program on-chain and write the result to `program_state`.
/// Shared between the unverify and pda webhooks.
pub(crate) async fn refresh_state(db: &Db, program_id: &str) -> Result<()> {
    let pid = Pubkey::from_str(program_id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let mut snaps = snapshot_programs(&[pid]).await?;
    if let Some(snap) = snaps.remove(&pid) {
        db.upsert_program_state(program_id, &snap).await?;
        info!("refreshed state for {}", program_id);
    }
    Ok(())
}
