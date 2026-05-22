//! `GET /logs/:build_id` — stdout/stderr from a build.

use crate::{
    db::Db,
    error::{ApiError, Result},
    logs,
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use std::str::FromStr;
use uuid::Uuid;

pub async fn fetch(
    State(db): State<Db>,
    Path(build_id): Path<String>,
) -> Result<Json<Value>> {
    let id = Uuid::from_str(&build_id)
        .map_err(|_| ApiError::BadRequest("Invalid build id (expected UUID)".into()))?;
    let Some(file) = db.get_build_log_file(id).await? else {
        return Ok(Json(
            json!({ "error": "We could not find the logs for this build" }),
        ));
    };
    Ok(Json(logs::read_logs(&file).await))
}
