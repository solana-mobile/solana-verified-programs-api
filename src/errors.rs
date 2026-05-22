//! Single error enum the whole API returns. The wire body is always
//! `{"status": "error", "error": "<msg>"}`.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    /// Malformed input — 400.
    #[error("{0}")]
    BadRequest(String),

    /// Missing — 404.
    #[error("{0}")]
    NotFound(String),

    #[error(transparent)]
    Db(#[from] sqlx::Error),

    /// RPC failure (or all configured RPCs were rate-limited).
    #[error("rpc: {0}")]
    Rpc(String),

    /// `solana-verify` exited non-zero; the message carries its stdout.
    #[error("build failed: {0}")]
    Build(String),

    #[error("{0}")]
    Internal(String),
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "status": "error", "error": self.to_string() });
        (self.status(), Json(body)).into_response()
    }
}

impl From<solana_client::client_error::ClientError> for ApiError {
    fn from(err: solana_client::client_error::ClientError) -> Self {
        ApiError::Rpc(err.to_string())
    }
}

impl From<solana_pubkey::ParsePubkeyError> for ApiError {
    fn from(err: solana_pubkey::ParsePubkeyError) -> Self {
        ApiError::BadRequest(format!("invalid pubkey: {err}"))
    }
}

impl From<solana_account_decoder::parse_account_data::ParseAccountError> for ApiError {
    fn from(err: solana_account_decoder::parse_account_data::ParseAccountError) -> Self {
        ApiError::Rpc(err.to_string())
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        ApiError::Internal(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;
