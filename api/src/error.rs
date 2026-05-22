use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    #[error(transparent)]
    Db(#[from] sqlx::Error),

    #[error("rpc: {0}")]
    Rpc(String),

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

impl From<solana_sdk::pubkey::ParsePubkeyError> for ApiError {
    fn from(err: solana_sdk::pubkey::ParsePubkeyError) -> Self {
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
