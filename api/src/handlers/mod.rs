pub mod read;
pub mod system;
pub mod verify;
pub mod webhooks;

use crate::config::CONFIG;
use axum::http::HeaderMap;

pub fn is_authorized(headers: &HeaderMap) -> bool {
    headers
        .get("AUTHORIZATION")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == CONFIG.auth_secret)
}
