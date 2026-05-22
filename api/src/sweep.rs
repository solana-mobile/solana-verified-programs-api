//! Periodic refresh of `program_state` rows — the slow path; webhooks are
//! the fast path.

use crate::{
    config::CONFIG,
    db::Db,
    onchain::{get_on_chain_hash, get_program_state},
};
use futures::{stream, StreamExt};
use solana_sdk::pubkey::Pubkey;
use std::{str::FromStr, time::Duration};
use tracing::{error, info};

const BATCH_SIZE: i64 = 200;
const CONCURRENCY: usize = 16;

pub fn spawn(db: Db) {
    tokio::spawn(async move {
        let interval = Duration::from_secs(CONFIG.sweep_interval_seconds);
        let mut ticker = tokio::time::interval(interval);
        info!(
            "sweep loop started, interval={}s",
            CONFIG.sweep_interval_seconds
        );
        loop {
            ticker.tick().await;
            if let Err(e) = run_once(&db).await {
                error!("sweep cycle: {}", e);
            }
        }
    });
}

async fn run_once(db: &Db) -> crate::error::Result<()> {
    let ids = db.oldest_program_states(BATCH_SIZE).await?;
    if ids.is_empty() {
        info!("sweep: no rows yet");
        return Ok(());
    }
    info!("sweep: refreshing {} programs", ids.len());

    stream::iter(ids)
        .for_each_concurrent(CONCURRENCY, |program_id| {
            let db = db.clone();
            async move {
                if let Err(e) = refresh_one(&db, &program_id).await {
                    error!("refresh {}: {}", program_id, e);
                }
            }
        })
        .await;
    Ok(())
}

async fn refresh_one(db: &Db, program_id: &str) -> crate::error::Result<()> {
    let pid = Pubkey::from_str(program_id)
        .map_err(|e| crate::error::ApiError::BadRequest(e.to_string()))?;
    let state = get_program_state(&pid).await.unwrap_or_else(|e| {
        // Default to "not closed, not frozen" on transient errors so we don't
        // overwrite stale data with a hard-coded closed flag.
        let _ = e;
        crate::onchain::ProgramOnchainState {
            authority: None,
            is_frozen: false,
            is_closed: false,
        }
    });

    let on_chain_hash = if state.is_closed {
        None
    } else {
        match get_on_chain_hash(program_id).await {
            Ok(h) => Some(h),
            Err(e) => {
                if e.to_string().contains("Program appears to be closed") {
                    db.mark_closed(program_id).await?;
                    return Ok(());
                }
                None
            }
        }
    };

    db.upsert_program_state(program_id, on_chain_hash.as_deref(), &state)
        .await?;
    Ok(())
}
