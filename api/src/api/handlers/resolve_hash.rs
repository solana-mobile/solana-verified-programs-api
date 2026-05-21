use crate::db::models::{ApiResponse, ErrorResponse, ResolveHashResponse, Status};
use crate::db::DbClient;
use crate::validation::validate_executable_hash;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Deserialize, Serialize)]
pub struct ResolveHashParams {
    pub hash: String,
}

/// `GET /resolve-hash/:hash` — every signer's claim about the build provenance
/// of these bytes. Empty list (200, `[]`) when no signer has ever claimed this
/// hash; 400 on a malformed hash.
///
/// Trust filtering is the consumer's job: each row carries the signer that
/// made the claim. For program-bound verification consumers want, see
/// `/status/:program_id`, which applies the trust filter for them.
pub(crate) async fn resolve_hash(
    State(db): State<DbClient>,
    Path(ResolveHashParams { hash }): Path<ResolveHashParams>,
) -> (StatusCode, Json<ApiResponse>) {
    if let Err(message) = validate_executable_hash(&hash) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                ErrorResponse {
                    status: Status::Error,
                    error: message,
                }
                .into(),
            ),
        );
    }

    info!("Resolving build claims for executable_hash: {}", hash);

    match db.get_verified_hashes_by_hash(&hash).await {
        Ok(rows) => {
            let claims: Vec<ResolveHashResponse> = rows.into_iter().map(Into::into).collect();
            (StatusCode::OK, Json(ApiResponse::ResolveHashList(claims)))
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                ErrorResponse {
                    status: Status::Error,
                    error: "Database lookup failed".to_string(),
                }
                .into(),
            ),
        ),
    }
}
