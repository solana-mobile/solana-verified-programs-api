//! Periodic refresh of `program_state` rows — the slow path; webhooks are
//! the fast path. Each cycle covers every row in `program_state` via batched
//! `getMultipleAccounts` calls.

use crate::{config::CONFIG, db::Db, onchain::snapshot_programs};
use solana_sdk::pubkey::Pubkey;
use std::{str::FromStr, time::Duration};
use tracing::{error, info};

/// Spawns the sweep task. Runs for the process's lifetime.
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
    let ids = db.sweep_program_ids().await?;
    if ids.is_empty() {
        return Ok(());
    }
    let pubkeys: Vec<Pubkey> = ids
        .iter()
        .filter_map(|s| Pubkey::from_str(s).ok())
        .collect();
    info!("sweep: refreshing {} programs", pubkeys.len());

    let snapshots = snapshot_programs(&pubkeys).await?;
    for (pid, snap) in &snapshots {
        let pid_str = pid.to_string();
        if let Err(e) = db.upsert_program_state(&pid_str, snap).await {
            error!("upsert state for {}: {}", pid_str, e);
        }
    }
    info!("sweep: applied {} snapshots", snapshots.len());
    Ok(())
}
