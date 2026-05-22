//! Periodic refresh of `program_state` rows — the slow path; webhooks are
//! the fast path. Each cycle covers every row in `program_state` via batched
//! `getMultipleAccounts` calls.

use crate::{CONFIG, db::Db, onchain::snapshot_programs};
use solana_pubkey::Pubkey;
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

/// Health view for the `/health/background-jobs` endpoint, derived from
/// the timestamp on the oldest `program_state` row.
pub struct BackgroundJobManager<'a> {
    db: &'a Db,
}

impl<'a> BackgroundJobManager<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub async fn get_health_status(&self) -> crate::response::BackgroundJobHealth {
        let last = self.db.last_sweep_at().await.ok().flatten();
        let now = chrono::Utc::now();
        let interval = chrono::Duration::seconds(CONFIG.sweep_interval_seconds as i64);
        match last {
            Some(t) => {
                let lag = now - t;
                if lag > interval * 2 {
                    crate::response::BackgroundJobHealth {
                        status: "Inactive".into(),
                        last_program_check: Some(t.naive_utc()),
                        message: format!(
                            "Last sweep was {}s ago, expected interval {}s",
                            lag.num_seconds(),
                            interval.num_seconds()
                        ),
                    }
                } else {
                    crate::response::BackgroundJobHealth {
                        status: "Active".into(),
                        last_program_check: Some(t.naive_utc()),
                        message: "Background sweep running normally".into(),
                    }
                }
            }
            None => crate::response::BackgroundJobHealth {
                status: "unknown".into(),
                last_program_check: None,
                message: "no program_state rows yet".into(),
            },
        }
    }
}

async fn run_once(db: &Db) -> crate::errors::Result<()> {
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
