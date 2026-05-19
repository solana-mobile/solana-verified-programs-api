use crate::db::DbClient;
use axum::{extract::State, http::StatusCode, Json};

/// Health check: Postgres + Redis reachable?
pub async fn health_check(State(db): State<DbClient>) -> (StatusCode, Json<serde_json::Value>) {
    let redis_ok = db.get_async_redis_conn().await.is_ok();
    let db_ok = db.get_db_conn().await.is_ok();
    let overall_ok = redis_ok && db_ok;

    let body = serde_json::json!({
        "status": if overall_ok { "ok" } else { "degraded" },
        "database": if db_ok { "connected" } else { "error" },
        "redis": if redis_ok { "connected" } else { "error" },
        "timestamp": chrono::Utc::now(),
    });

    let code = if overall_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}
