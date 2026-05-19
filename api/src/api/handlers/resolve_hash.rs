use crate::db::models::{ErrorResponse, ResolveHashResponse, Status};
use crate::db::DbClient;
use crate::validation::validate_executable_hash;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Path params for `/resolve-hash/:hash`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ResolveHashParams {
    pub hash: String,
}

/// Handler for `GET /resolve-hash/:hash`.
///
/// Returns the `(repository, commit, build_args)` triple that deterministically
/// produces `hash`, or 404 if no verified build is known for this hash.
///
/// This is the hot path of the content-addressed directory:
/// consumers hash whatever bytes they have (deployed program, buffer, local `.so`)
/// and query the same way.
pub(crate) async fn resolve_hash(
    State(db): State<DbClient>,
    Path(ResolveHashParams { hash }): Path<ResolveHashParams>,
) -> impl IntoResponse {
    if let Err(message) = validate_executable_hash(&hash) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::to_value(ErrorResponse {
                status: Status::Error,
                error: message,
            }).unwrap_or_else(|_| serde_json::Value::Null)),
        );
    }

    info!("Resolving build for executable_hash: {}", hash);

    match db.get_verified_hash(&hash).await {
        Ok(Some(entry)) => {
            let response: ResolveHashResponse = entry.into();
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap_or_else(|_| serde_json::Value::Null)),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::to_value(ErrorResponse {
                status: Status::Error,
                error: format!("No verified build is known for hash {hash}"),
            }).unwrap_or_else(|_| serde_json::Value::Null)),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::to_value(ErrorResponse {
                status: Status::Error,
                error: "Database lookup failed".to_string(),
            }).unwrap_or_else(|_| serde_json::Value::Null)),
        ),
    }
}
