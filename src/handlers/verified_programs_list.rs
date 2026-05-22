//! `GET /verified-programs` and `GET /verified-programs/:page`.

use crate::{
    db::{Db, PER_PAGE},
    response::{PaginationMeta, VerifiedProgramListResponse},
    validation,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub search: Option<String>,
}

pub async fn list(
    State(db): State<Db>,
    Query(q): Query<ListQuery>,
) -> (StatusCode, Json<VerifiedProgramListResponse>) {
    paginated(State(db), Path(1), Query(q)).await
}

pub async fn paginated(
    State(db): State<Db>,
    Path(page): Path<i64>,
    Query(q): Query<ListQuery>,
) -> (StatusCode, Json<VerifiedProgramListResponse>) {
    let page = page.max(1);
    let search = q.search.as_deref().unwrap_or("").trim();
    if let Err(msg) = validation::validate_search(search) {
        return (
            StatusCode::BAD_REQUEST,
            Json(VerifiedProgramListResponse {
                meta: empty_meta(page),
                verified_programs: vec![],
                error: Some(msg),
            }),
        );
    }

    match db.verified_programs_page(page, search).await {
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
                meta: empty_meta(page),
                verified_programs: vec![],
                error: None,
            }),
        ),
    }
}

fn empty_meta(page: i64) -> PaginationMeta {
    PaginationMeta {
        total: 0,
        page,
        total_pages: 0,
        items_per_page: PER_PAGE,
        has_next_page: false,
        has_prev_page: false,
    }
}
