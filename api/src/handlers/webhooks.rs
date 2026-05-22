use crate::{
    build,
    db::Db,
    handlers::is_authorized,
    onchain::{snapshot_programs, OtterBuildParams, OTTER_VERIFY_PROGRAM_ID},
    rpc::rpc,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use borsh::BorshDeserialize;
use serde::Deserialize;
use serde_json::Value;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use tracing::{error, info, warn};

const UPGRADE_INSTRUCTION_DATA: &str = "5Sxr3";

#[derive(Debug, Deserialize)]
#[allow(dead_code, non_snake_case)]
pub struct HeliusParsedTransaction {
    pub description: String,
    #[serde(rename = "type")]
    pub instruction_type: String,
    pub source: String,
    pub fee: u64,
    pub feePayer: String,
    pub signature: String,
    pub slot: u64,
    pub timestamp: u64,
    pub tokenTransfers: Vec<Value>,
    pub nativeTransfers: Vec<Value>,
    pub accountData: Vec<Value>,
    pub transactionError: Option<String>,
    pub instructions: Vec<Instruction>,
    pub events: Value,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code, non_snake_case)]
pub struct Instruction {
    pub accounts: Vec<String>,
    pub data: String,
    pub programId: String,
    pub innerInstructions: Vec<Value>,
}

fn parse_helius(payload: &[Value]) -> Result<HeliusParsedTransaction, (StatusCode, &'static str)> {
    if payload.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Empty payload"));
    }
    serde_json::from_value(payload[0].clone())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid payload"))
}

pub async fn unverify(
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

async fn refresh_state(db: &Db, program_id: &str) -> crate::error::Result<()> {
    let pid = Pubkey::from_str(program_id)
        .map_err(|e| crate::error::ApiError::BadRequest(e.to_string()))?;
    let mut snaps = snapshot_programs(&[pid]).await?;
    if let Some(snap) = snaps.remove(&pid) {
        db.upsert_program_state(program_id, &snap).await?;
        info!("refreshed state for {}", program_id);
    }
    Ok(())
}

pub async fn pda(
    State(db): State<Db>,
    headers: HeaderMap,
    Json(payload): Json<Vec<Value>>,
) -> (StatusCode, &'static str) {
    if !is_authorized(&headers) {
        warn!("unauthorized /pda call");
        return (
            StatusCode::UNAUTHORIZED,
            "Missing or invalid authorization header",
        );
    }
    let tx = match parse_helius(&payload) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let otter_id = OTTER_VERIFY_PROGRAM_ID.to_string();
    tokio::spawn(async move {
        for ix in tx.instructions {
            if ix.programId != otter_id {
                continue;
            }
            if ix.accounts.len() < 3 {
                continue;
            }
            let pda_account = ix.accounts[0].clone();
            let program_id = ix.accounts[2].clone();
            if let Err(e) = process_pda(&db, &program_id, &pda_account).await {
                error!("pda {} program {}: {}", pda_account, program_id, e);
            }
        }
    });
    (StatusCode::OK, "PDA updates/creations request received")
}

async fn process_pda(db: &Db, program_id: &str, pda_account: &str) -> crate::error::Result<()> {
    refresh_state(db, program_id).await.ok();

    let pda_pubkey = Pubkey::from_str(pda_account)
        .map_err(|e| crate::error::ApiError::BadRequest(e.to_string()))?;
    let data = rpc()
        .run(|client| {
            let pda = pda_pubkey;
            async move {
                client
                    .get_account_data(&pda)
                    .await
                    .map_err(|e| crate::error::ApiError::Rpc(format!("get_account_data: {e}")))
            }
        })
        .await?;
    let otter = OtterBuildParams::try_from_slice(&data[8..])
        .map_err(|e| crate::error::ApiError::Internal(format!("deserialize PDA: {e}")))?;

    let new_build = crate::db::NewBuild {
        repository: otter.git_url.clone(),
        commit_hash: Some(otter.commit.clone()),
        program_id: otter.address.to_string(),
        lib_name: otter.library_name(),
        base_docker_image: otter.base_image(),
        mount_path: otter.mount_path(),
        cargo_args: otter.cargo_args(),
        bpf_flag: otter.bpf(),
        arch: otter.arch(),
        signer: Some(otter.signer.to_string()),
    };

    if let Some(dup) = db.find_duplicate(&new_build).await? {
        if dup.status == "in_progress" || dup.status == "completed" {
            info!("dup build for {} from PDA — skipping kick", program_id);
            return Ok(());
        }
    }

    let id = db.insert_build(&new_build).await?;
    let db2 = db.clone();
    tokio::spawn(async move {
        build::execute(id, new_build, db2, None).await;
    });
    Ok(())
}
