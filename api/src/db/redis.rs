use super::DbClient;
use crate::errors::ApiError;
use crate::Result;
use redis::{AsyncCommands, FromRedisValue, Value};
use tracing::{error, info};

/// Redis cache expiry times in seconds
const DEFAULT_CACHE_EXPIRY_SECONDS: u64 = 5 * 60; // 5 minutes for general cache
pub const PROGRAM_AUTHORITY_CACHE_EXPIRY_SECONDS: u64 = 60 * 60; // 1 hour for program authorities

/// DbClient helper functions for Redis cache to set and retrieve cache values
impl DbClient {
    /// Sets a value in Redis cache with default expiry
    pub async fn set_cache(&self, program_address: &str, value: &str) -> Result<()> {
        self.set_cache_with_expiry(program_address, value, DEFAULT_CACHE_EXPIRY_SECONDS)
            .await
    }

    /// Sets a value in Redis cache with custom expiry
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

        info!(
            "Cache set for key: {} with expiry: {}s",
            key, expiry_seconds
        );
        Ok(())
    }

    /// Retrieves a value from Redis cache
    pub async fn get_cache(&self, program_address: &str) -> Result<String> {
        let mut redis_conn = self.get_async_redis_conn().await.map_err(|err| {
            error!("Redis connection error: {}", err);
            ApiError::from(err)
        })?;

        let value: Value = redis_conn.get(program_address).await.map_err(|err| {
            error!("Redis GET failed: {}", err);
            ApiError::from(err)
        })?;

        match value {
            Value::Nil => Err(ApiError::NotFound(format!(
                "Cache record not found for program: {program_address}"
            ))),
            _ => FromRedisValue::from_redis_value(&value).map_err(|err| {
                error!("Redis value conversion error: {}", err);
                ApiError::from(err)
            }),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires database and Redis"]
    async fn test_cache_operations() {
        dotenv::dotenv().ok();
        let db_url = std::env::var("TEST_DATABASE_URL").unwrap();
        let redis_url = std::env::var("TEST_REDIS_URL").unwrap();
        let client = DbClient::new(&db_url, &redis_url);

        let program = "test_program";
        let hash = "test_hash";

        // Test set
        let set_result = client.set_cache(program, hash).await;
        assert!(set_result.is_ok());

        // Test get
        let get_result = client.get_cache(program).await;
        assert!(get_result.is_ok());
        assert_eq!(get_result.unwrap(), hash);
    }
}
