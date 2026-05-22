//! `POST /unverify` — Helius webhook for BPF upgrade transactions.

use crate::{
    db::Db,
    handlers::{is_authorized, parse_helius, refresh_state},
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde_json::Value;
use tracing::{error, warn};

const UPGRADE_INSTRUCTION_DATA: &str = "5Sxr3";

pub async fn handle_unverify(
    State(db): State<Db>,
    headers: HeaderMap,
    Json(payload): Json<Vec<Value>>,
) -> (StatusCode, &'static str) {
    if !is_authorized(&headers) {
        warn!("unauthorized /unverify call");
        return (
            StatusCode::UNAUTHORIZED,
            "Missing or invalid authorization header",
        );
    }
    let tx = match parse_helius(&payload) {
        Ok(t) => t,
        Err(e) => return e,
    };
    tokio::spawn(async move {
        for ix in tx.instructions {
            if ix.data == UPGRADE_INSTRUCTION_DATA {
                let program_id = &ix.accounts[1];
                if let Err(e) = refresh_state(&db, program_id).await {
                    error!("refresh state for {}: {}", program_id, e);
                }
            }
        }
    });
    (StatusCode::OK, "Unverify request received")
}
