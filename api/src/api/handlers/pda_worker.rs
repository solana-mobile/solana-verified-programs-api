use std::str::FromStr;

use crate::{
    api::handlers::{async_verify::process_verification, is_authorized},
    db::{
        models::{parse_helius_transaction, SolanaProgramBuildParams},
        DbClient,
    },
    services::{onchain::OtterBuildParams, rpc_manager::get_rpc_manager},
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use borsh::BorshDeserialize;
use serde_json::Value;
use solana_sdk::pubkey::Pubkey;
use tracing::{error, info, warn};

const OTTER_VERIFY_PROGRAM_ID: &str = "verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC";

pub(crate) async fn handle_pda_updates_creations(
    State(db): State<DbClient>,
    headers: HeaderMap,
    Json(payload): Json<Vec<Value>>,
) -> (StatusCode, &'static str) {
    info!("Received PDA updates/creation event");

    if !is_authorized(&headers) {
        warn!("Unauthorized /pda attempt");
        return (
            StatusCode::UNAUTHORIZED,
            "Missing or invalid authorization header",
        );
    }

    let parsed = match parse_helius_transaction(&payload) {
        Ok(p) => p,
        Err(status) => return status,
    };

    let db_for_task = db.clone();
    tokio::spawn(async move {
        for ix in parsed.instructions {
            if ix.programId != OTTER_VERIFY_PROGRAM_ID {
                continue;
            }
            if ix.accounts.len() < 3 {
                continue;
            }
            let pda_account = ix.accounts[0].clone();
            let program_id = ix.accounts[2].clone();
            if let Err(e) =
                process_pda_event(&db_for_task, &program_id, &pda_account).await
            {
                error!(
                    "Failed to process PDA event for program {}: {:?}",
                    program_id, e
                );
            }
        }
    });

    (StatusCode::OK, "PDA updates/creations request received")
}

async fn process_pda_event(
    db: &DbClient,
    program_id: &str,
    pda_account: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pda_pubkey = Pubkey::from_str(pda_account)?;

    let rpc_manager = get_rpc_manager();
    let pda_data = rpc_manager
        .execute_with_retry(|client| async move {
            client
                .get_account_data(&pda_pubkey)
                .await
                .map_err(|e| crate::errors::ApiError::Custom(format!("RPC error: {e}")))
        })
        .await?;

    let otter_build_params = OtterBuildParams::try_from_slice(&pda_data[8..])?;
    let signer = otter_build_params.signer.to_string();
    let build_params = SolanaProgramBuildParams::from(otter_build_params);

    info!(
        "PDA event: triggering verification for program {} (signer {})",
        program_id, signer
    );
    let _ = process_verification(db.clone(), build_params, signer, None).await;
    Ok(())
}
