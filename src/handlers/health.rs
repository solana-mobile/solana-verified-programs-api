use crate::{db::Db, sweep::BackgroundJobManager};
use axum::{Json, extract::State, http::StatusCode};

/// Health check endpoint that includes background job status
pub async fn health_check(State(db): State<Db>) -> (StatusCode, Json<serde_json::Value>) {
    let bg_manager = BackgroundJobManager::new(&db);
    let bg_health = bg_manager.get_health_status().await;
    let bg_ok = bg_health.status == "Active";

    // Get database connection and status
    let (db_status, db_ok) = match db.ping().await {
        Ok(_) => (serde_json::json!("connected"), true),
        Err(e) => (
            serde_json::json!({
                "status": "error",
                "message": e.to_string()
            }),
            false,
        ),
    };

    let overall_ok = bg_ok && db_ok;

    let health_status = serde_json::json!({
        "status": if overall_ok { "ok" } else { "degraded" },
        "database": db_status,
        "background_jobs": bg_health,
        "timestamp": chrono::Utc::now()
    });

    let status_code = if overall_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(health_status))
}

/// Background job status endpoint
pub async fn background_job_status(
    State(db): State<Db>,
) -> (StatusCode, Json<crate::response::BackgroundJobHealth>) {
    let bg_manager = BackgroundJobManager::new(&db);
    let health = bg_manager.get_health_status().await;

    let status_code = match health.status.as_str() {
        "healthy" => StatusCode::OK,
        "unknown" => StatusCode::ACCEPTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };

    (status_code, Json(health))
}
