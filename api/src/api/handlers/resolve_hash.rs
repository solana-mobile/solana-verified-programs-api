use crate::db::models::{ErrorResponse, Status};
use crate::db::DbClient;
use crate::validation::validate_executable_hash;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::Value;
use tracing::{error, info};

/// Resolves an executable hash to every signer's verified-build claim about it.
///
/// # Endpoint: GET /resolve-hash/:hash
pub(crate) async fn resolve_hash(
    State(db): State<DbClient>,
    Path(hash): Path<String>,
) -> (StatusCode, Json<Value>) {
    if let Err(message) = validate_executable_hash(&hash) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(ErrorResponse {
                    status: Status::Error,
                    error: message,
                })
                .unwrap_or(Value::Null),
            ),
        );
    }

    info!("resolve-hash: {}", hash);
    match db.get_verified_builds_by_hash(&hash).await {
        Ok(claims) => (
            StatusCode::OK,
            Json(serde_json::to_value(claims).unwrap_or(Value::Array(vec![]))),
        ),
        Err(e) => {
            error!("Failed to load verified builds for {}: {:?}", hash, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::to_value(ErrorResponse {
                        status: Status::Error,
                        error: "Database lookup failed".to_string(),
                    })
                    .unwrap_or(Value::Null),
                ),
            )
        }
    }
}
