//! GET handlers — pure DB reads. Freshness is the sweep + webhooks' job.

use crate::{
    db::{BuildRow, Db, ProgramStateRow, PER_PAGE},
    error::{ApiError, Result},
    logs,
    onchain::build_repo_url,
    response::{
        BackgroundJobHealth, ExtendedStatusResponse, JobVerificationResponse, PaginationMeta,
        ResolveHashEntry, ResolveHashResponse, Status, StatusResponse, VerificationResponse,
        VerificationResponseWithSigner, VerifiedProgramListResponse, VerifiedProgramStatusResponse,
        VerifiedProgramsStatusListResponse,
    },
    types::{ProgramId, SearchQuery},
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::str::FromStr;
use tracing::info;
use uuid::Uuid;

/// `search` is parsed into a [`SearchQuery`] inside the handler so the
/// rejection shape matches the rest of the listing response.
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub search: Option<String>,
}

/// `GET /status/:program_id` — current verification status for one program.
pub async fn status(
    State(db): State<Db>,
    Path(program_id): Path<ProgramId>,
) -> Json<ExtendedStatusResponse> {
    info!("/status {}", program_id);
    let state = db
        .get_program_state(&program_id.as_str())
        .await
        .ok()
        .flatten();
    let on_chain_hash = state
        .as_ref()
        .and_then(|s| s.on_chain_hash.clone())
        .unwrap_or_default();
    let (is_frozen, is_closed) = state
        .as_ref()
        .map(|s| (s.is_frozen, s.is_closed))
        .unwrap_or((false, false));

    let build = db
        .latest_completed_build(&program_id, Some(&on_chain_hash))
        .await
        .ok()
        .flatten();

    let resp = match build {
        Some(b) => {
            let is_verified = !on_chain_hash.is_empty()
                && b.executable_hash.as_deref() == Some(on_chain_hash.as_str())
                && !is_closed;
            ExtendedStatusResponse {
                status: StatusResponse {
                    is_verified,
                    message: status_message(is_verified),
                    on_chain_hash,
                    executable_hash: b.executable_hash.unwrap_or_default(),
                    repo_url: build_repo_url(&b.repository, b.commit_hash.as_deref()),
                    commit: b.commit_hash.unwrap_or_default(),
                    last_verified_at: b.completed_at.map(|t| t.naive_utc()),
                },
                is_frozen,
                is_closed,
            }
        }
        None => ExtendedStatusResponse {
            status: StatusResponse {
                is_verified: false,
                message: "On chain program not verified".into(),
                on_chain_hash,
                executable_hash: String::new(),
                repo_url: String::new(),
                commit: String::new(),
                last_verified_at: None,
            },
            is_frozen,
            is_closed,
        },
    };
    Json(resp)
}

/// `GET /status-all/:program_id` — one entry per signer that has claimed the program.
pub async fn status_all(
    State(db): State<Db>,
    Path(program_id): Path<ProgramId>,
) -> Result<Json<Vec<VerificationResponseWithSigner>>> {
    info!("/status-all {}", program_id);
    let state = db.get_program_state(&program_id.as_str()).await?;
    let on_chain_hash = state
        .as_ref()
        .and_then(|s| s.on_chain_hash.clone())
        .unwrap_or_default();
    let is_frozen = state.as_ref().map(|s| s.is_frozen).unwrap_or(false);
    let is_closed = state.as_ref().map(|s| s.is_closed).unwrap_or(false);

    let builds = db.completed_builds_by_signer(&program_id).await?;
    let out = builds
        .into_iter()
        .map(|b| build_with_signer(b, &on_chain_hash, is_frozen, is_closed))
        .collect();
    Ok(Json(out))
}

fn build_with_signer(
    b: BuildRow,
    on_chain_hash: &str,
    is_frozen: bool,
    is_closed: bool,
) -> VerificationResponseWithSigner {
    let is_verified = !on_chain_hash.is_empty()
        && b.executable_hash.as_deref() == Some(on_chain_hash)
        && !is_closed;
    VerificationResponseWithSigner {
        signer: b.signer.clone().unwrap_or_default(),
        verification_response: VerificationResponse {
            is_verified,
            on_chain_hash: on_chain_hash.to_string(),
            executable_hash: b.executable_hash.unwrap_or_default(),
            repo_url: build_repo_url(&b.repository, b.commit_hash.as_deref()),
            commit: b.commit_hash.unwrap_or_default(),
            last_verified_at: b.completed_at.map(|t| t.naive_utc()),
            is_frozen,
            is_closed,
        },
    }
}

/// `GET /resolve-hash/:hash` — every completed build that produced the hash.
pub async fn resolve_hash(
    State(db): State<Db>,
    Path(hash): Path<String>,
) -> Result<Json<ResolveHashResponse>> {
    info!("/resolve-hash {}", hash);
    let hash = hash.trim().to_string();
    if hash.is_empty() {
        return Err(ApiError::BadRequest("hash cannot be empty".into()));
    }
    let builds = db.builds_by_executable_hash(&hash).await?;
    let mut entries = Vec::with_capacity(builds.len());
    for b in builds {
        let is_current = db
            .get_program_state(&b.program_id)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.on_chain_hash)
            .as_deref()
            == Some(hash.as_str());
        entries.push(ResolveHashEntry {
            build_id: b.id.to_string(),
            program_id: b.program_id,
            signer: b.signer,
            repository: b.repository,
            commit: b.commit_hash,
            completed_at: b.completed_at.map(|t| t.naive_utc()),
            is_currently_on_chain: is_current,
        });
    }
    Ok(Json(ResolveHashResponse {
        executable_hash: hash,
        builds: entries,
    }))
}

/// `GET /job/:job_id` — async build status.
pub async fn job(
    State(db): State<Db>,
    Path(job_id): Path<String>,
) -> Result<Json<JobVerificationResponse>> {
    let id = Uuid::from_str(&job_id)
        .map_err(|_| ApiError::BadRequest("invalid job id (expected UUID)".into()))?;
    let build = db
        .get_build(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("job {job_id} not found")))?;
    let resp = match build.status.as_str() {
        "completed" => JobVerificationResponse {
            status: "completed".into(),
            message: "Job completed".into(),
            on_chain_hash: db
                .get_program_state(&build.program_id)
                .await
                .ok()
                .flatten()
                .and_then(|s| s.on_chain_hash)
                .unwrap_or_default(),
            executable_hash: build.executable_hash.unwrap_or_default(),
            repo_url: build_repo_url(&build.repository, build.commit_hash.as_deref()),
        },
        "failed" => JobVerificationResponse {
            status: "failed".into(),
            message: build
                .error_message
                .unwrap_or_else(|| "Verification failed".into()),
            on_chain_hash: String::new(),
            executable_hash: String::new(),
            repo_url: String::new(),
        },
        _ => JobVerificationResponse {
            status: "in_progress".into(),
            message: "Please wait, the verification is in progress".into(),
            on_chain_hash: String::new(),
            executable_hash: String::new(),
            repo_url: String::new(),
        },
    };
    Ok(Json(resp))
}

/// `GET /logs/:build_id` — stdout/stderr from a build.
pub async fn build_logs(State(db): State<Db>, Path(build_id): Path<String>) -> Result<Json<Value>> {
    let id = Uuid::from_str(&build_id)
        .map_err(|_| ApiError::BadRequest("Invalid build id (expected UUID)".into()))?;
    let Some(file) = db.get_build_log_file(id).await? else {
        return Ok(Json(
            json!({ "error": "We could not find the logs for this build" }),
        ));
    };
    Ok(Json(logs::read(&file).await))
}

/// `GET /verified-programs` — first page of the verified-programs list.
pub async fn verified_programs(
    State(db): State<Db>,
    Query(q): Query<ListQuery>,
) -> (StatusCode, Json<VerifiedProgramListResponse>) {
    verified_programs_paginated(State(db), Path(1), Query(q)).await
}

/// `GET /verified-programs/:page` — paginated verified-programs list.
pub async fn verified_programs_paginated(
    State(db): State<Db>,
    Path(page): Path<i64>,
    Query(q): Query<ListQuery>,
) -> (StatusCode, Json<VerifiedProgramListResponse>) {
    let page = page.max(1);
    let search = match q.search.as_deref() {
        Some(s) => match SearchQuery::from_str(s) {
            Ok(v) => v,
            Err(msg) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(VerifiedProgramListResponse {
                        meta: meta(0, page, PER_PAGE),
                        verified_programs: vec![],
                        error: Some(msg),
                    }),
                );
            }
        },
        None => SearchQuery::from_str("").unwrap(),
    };

    match db.verified_programs_page(page, search.as_str()).await {
        Ok((ids, total)) => {
            let total_pages = (total + PER_PAGE - 1) / PER_PAGE;
            (
                StatusCode::OK,
                Json(VerifiedProgramListResponse {
                    meta: PaginationMeta {
                        total,
                        page,
                        total_pages,
                        items_per_page: PER_PAGE,
                        has_next_page: page < total_pages,
                        has_prev_page: page > 1,
                    },
                    verified_programs: ids,
                    error: None,
                }),
            )
        }
        Err(_) => (
            StatusCode::OK,
            Json(VerifiedProgramListResponse {
                meta: meta(0, page, PER_PAGE),
                verified_programs: vec![],
                error: None,
            }),
        ),
    }
}

/// `GET /verified-programs-status` — verification status for every verified program.
pub async fn verified_programs_status(
    State(db): State<Db>,
) -> (StatusCode, Json<VerifiedProgramsStatusListResponse>) {
    match db.verified_programs_with_state().await {
        Ok(rows) => {
            let data = rows
                .into_iter()
                .map(|(b, s)| program_status_row(b, s))
                .collect();
            (
                StatusCode::OK,
                Json(VerifiedProgramsStatusListResponse {
                    status: Status::Success,
                    data: Some(data),
                    error: None,
                }),
            )
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(VerifiedProgramsStatusListResponse {
                status: Status::Error,
                data: None,
                error: Some("An unexpected database error occurred.".into()),
            }),
        ),
    }
}

fn program_status_row(b: BuildRow, s: ProgramStateRow) -> VerifiedProgramStatusResponse {
    let on_chain_hash = s.on_chain_hash.unwrap_or_default();
    let exec_hash = b.executable_hash.unwrap_or_default();
    let is_verified = !on_chain_hash.is_empty() && on_chain_hash == exec_hash;
    VerifiedProgramStatusResponse {
        program_id: b.program_id,
        is_verified,
        message: status_message(is_verified),
        on_chain_hash,
        executable_hash: exec_hash,
        last_verified_at: b.completed_at.map(|t| t.naive_utc()),
        repo_url: build_repo_url(&b.repository, b.commit_hash.as_deref()),
        commit: b.commit_hash.unwrap_or_default(),
    }
}

fn meta(total: i64, page: i64, per_page: i64) -> PaginationMeta {
    PaginationMeta {
        total,
        page,
        total_pages: 0,
        items_per_page: per_page,
        has_next_page: false,
        has_prev_page: false,
    }
}

fn status_message(is_verified: bool) -> String {
    if is_verified {
        "On chain program verified".into()
    } else {
        "On chain program not verified".into()
    }
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
