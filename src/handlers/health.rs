use crate::{db::Db, response::BackgroundJobHealth};
use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

/// `GET /health` — overall liveness: DB connectivity plus sweep recency.
pub async fn health_check(State(db): State<Db>) -> (StatusCode, Json<Value>) {
    let db_ok = db.ping().await.is_ok();
    let last = db.last_sweep_at().await.ok().flatten();
    let now = chrono::Utc::now();
    let interval = chrono::Duration::seconds(crate::config::CONFIG.sweep_interval_seconds as i64);
    let sweep_ok = match last {
        Some(t) => now - t <= interval * 2,
        None => true,
    };
    let overall = db_ok && sweep_ok;
    let body = json!({
        "status": if overall { "ok" } else { "degraded" },
        "database": if db_ok { "connected" } else { "error" },
        "sweep": {
            "last_program_check": last.map(|t| t.naive_utc()),
            "ok": sweep_ok
        },
        "timestamp": now
    });
    (
        if overall {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(body),
    )
}

/// `GET /health/background-jobs` — last sweep timestamp + liveness verdict.
pub async fn background_job_status(
    State(db): State<Db>,
) -> (StatusCode, Json<BackgroundJobHealth>) {
    let last = db.last_sweep_at().await.ok().flatten();
    let now = chrono::Utc::now();
    let interval = chrono::Duration::seconds(crate::config::CONFIG.sweep_interval_seconds as i64);

    let health = match last {
        Some(t) => {
            let lag = now - t;
            if lag > interval * 2 {
                BackgroundJobHealth {
                    status: "Inactive".into(),
                    last_program_check: Some(t.naive_utc()),
                    message: format!(
                        "Last sweep was {}s ago, expected interval {}s",
                        lag.num_seconds(),
                        interval.num_seconds()
                    ),
                }
            } else {
                BackgroundJobHealth {
                    status: "Active".into(),
                    last_program_check: Some(t.naive_utc()),
                    message: "Background sweep running normally".into(),
                }
            }
        }
        None => BackgroundJobHealth {
            status: "unknown".into(),
            last_program_check: None,
            message: "no program_state rows yet".into(),
        },
    };

    let code = match health.status.as_str() {
        "Active" => StatusCode::OK,
        "unknown" => StatusCode::ACCEPTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (code, Json(health))
}
