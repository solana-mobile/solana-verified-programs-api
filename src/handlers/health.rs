use crate::db::Db;
use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

/// `GET /health` — overall liveness: DB connectivity plus sweep recency.
pub async fn health(State(db): State<Db>) -> (StatusCode, Json<Value>) {
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
