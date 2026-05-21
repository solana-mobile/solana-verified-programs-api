use crate::db::DbClient;
use axum::{extract::State, http::StatusCode, Json};

/// Health check: Postgres + Redis reachable.
pub async fn health_check(State(db): State<DbClient>) -> (StatusCode, Json<serde_json::Value>) {
    // Get Redis connection and status
    let (redis_status, redis_ok) = match db.get_async_redis_conn().await {
        Err(e) => (
            serde_json::json!({
                "status": "error",
                "message": e.to_string()
            }),
            false,
        ),
        Ok(_) => (serde_json::json!("connected"), true),
    };

    // Get database connection and status
    let (db_status, db_ok) = match db.get_db_conn().await {
        Ok(_) => (serde_json::json!("connected"), true),
        Err(e) => (
            serde_json::json!({
                "status": "error",
                "message": e.to_string()
            }),
            false,
        ),
    };

    let overall_ok = redis_ok && db_ok;

    let health_status = serde_json::json!({
        "status": if overall_ok { "ok" } else { "degraded" },
        "database": db_status,
        "redis": redis_status,
        "timestamp": chrono::Utc::now()
    });

    let status_code = if overall_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(health_status))
}
