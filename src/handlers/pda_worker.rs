use std::str::FromStr;

use crate::{
    db::{Db, NewBuild},
    handlers::{async_verify::process_verification, is_authorized, parse_helius_transaction},
    onchain::{OTTER_VERIFY_PROGRAM_ID, OtterBuildParams, get_on_chain_hash},
    rpc::get_rpc_manager,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use borsh::BorshDeserialize;
use serde_json::Value;
use solana_pubkey::Pubkey;
use tracing::{error, info, warn};

pub(crate) async fn handle_pda_updates_creations(
    State(db): State<Db>,
    headers: HeaderMap,
    Json(payload): Json<Vec<Value>>,
) -> (StatusCode, &'static str) {
    info!("Received PDA updates/creation event");

    // Validate authorization
    if !is_authorized(&headers) {
        warn!("Unauthorized unverify attempt");
        return (
            StatusCode::UNAUTHORIZED,
            "Missing or invalid authorization header",
        );
    }

    // Validate payload
    let helius_parsed_transaction = match parse_helius_transaction(&payload) {
        Ok(parsed_transaction) => parsed_transaction,
        Err(status) => return status,
    };

    let otter_program_id = OTTER_VERIFY_PROGRAM_ID.to_string();
    // Process instructions
    for ix in helius_parsed_transaction.instructions {
        // Only process PDA updates/creations
        if ix.program_id != otter_program_id {
            continue;
        }
        let pda_account = &ix.accounts[0];
        let program_id = &ix.accounts[2];

        let _ = process_otter_verify_instruction(&db, program_id, pda_account).await;
    }

    (StatusCode::OK, "PDA updates/creations request received")
}

async fn process_otter_verify_instruction(
    db: &Db,
    program_id: &str,
    pda_account: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let executable_hash = match db.get_verified_build(program_id, None).await {
        Ok(data) => data.on_chain_hash,
        Err(_) => String::default(),
    };

    let onchain_hash = match get_on_chain_hash(program_id).await {
        Ok(hash) => hash,
        Err(e) => {
            let error_str = e.to_string();
            if error_str.contains("Program appears to be closed") {
                // Handle closed program using centralized helper
                db.handle_closed_program(program_id).await?;
                return Ok(()); // Exit early for closed programs
            }
            return Err(e.into());
        }
    };

    if onchain_hash != executable_hash {
        db.unverify_program(program_id, &onchain_hash).await?;
        // start new build
        let pda_account_pubkey = Pubkey::from_str(pda_account)?;
        let rpc_manager = get_rpc_manager();
        let params = rpc_manager
            .execute_with_retry(|client| async move {
                client
                    .get_account_data(&pda_account_pubkey)
                    .await
                    .map_err(|e| crate::errors::ApiError::Rpc(format!("RPC error: {e}")))
            })
            .await?;
        let otter_build_params = match OtterBuildParams::try_from_slice(&params[8..]) {
            Ok(params) => params,
            Err(e) => {
                error!("Failed to deserialize PDA data: {}", e);
                return Err(e.into());
            }
        };
        let signer = otter_build_params.signer.to_string();
        let new_build = NewBuild::from(&otter_build_params);
        let _ = process_verification(db.clone(), new_build, signer, None).await;
        info!("Successfully unverified program {}", program_id);
    } else {
        info!("Program {} has not been upgraded", program_id);
    }
    Ok(())
}
