use super::DbClient;
use crate::errors::ApiError;
use crate::Result;
use redis::AsyncCommands;
use tracing::{error, info};

/// Cache expiry for cached program-authority lookups (1 hour).
pub const PROGRAM_AUTHORITY_CACHE_EXPIRY_SECONDS: u64 = 60 * 60;

impl DbClient {
    /// Sets a value in Redis cache with the given expiry.
    pub async fn set_cache_with_expiry(
        &self,
        key: &str,
        value: &str,
        expiry_seconds: u64,
    ) -> Result<()> {
        let mut redis_conn = self.get_async_redis_conn().await.map_err(|err| {
            error!("Redis connection error: {}", err);
            ApiError::from(err)
        })?;

        let _: () = redis_conn
            .set_ex(key, value, expiry_seconds)
            .await
            .map_err(|err| {
                error!("Redis SET failed: {}", err);
                ApiError::from(err)
            })?;

        info!("Cache set for key: {} with expiry: {}s", key, expiry_seconds);
        Ok(())
    }
}
