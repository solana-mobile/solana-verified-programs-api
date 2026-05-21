use super::DbClient;
use crate::{db::models::SolanaProgramBuild, errors::ApiError, Result};
use diesel_async::RunQueryDsl;
use tracing::{error, info};

/// DbClient helper functions for SolanaProgramBuilds table
impl DbClient {
    /// Insert build params for a program
    pub async fn insert_build_params(&self, payload: &SolanaProgramBuild) -> Result<usize> {
        use crate::schema::solana_program_builds::dsl::*;

        let conn = &mut self.get_db_conn().await?;

        info!("Inserting build params for program: {}", payload.program_id);
        diesel::insert_into(solana_program_builds)
            .values(payload)
            .execute(conn)
            .await
            .map_err(|e| {
                error!("Failed to insert build params: {}", e);
                ApiError::Diesel(e)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    #[ignore = "requires database and Redis"]
    async fn test_build_params_operations() {
        dotenv::dotenv().ok();
        let db_url = std::env::var("TEST_DATABASE_URL").unwrap();
        let redis_url = std::env::var("TEST_REDIS_URL").unwrap();
        let client = DbClient::new(&db_url, &redis_url);

        // Create test build params
        let build = SolanaProgramBuild {
            id: "test_id".to_string(),
            repository: "test_repo".to_string(),
            program_id: "test_program".to_string(),
            commit_hash: Some("test_hash".to_string()),
            lib_name: None,
            base_docker_image: None,
            mount_path: None,
            cargo_args: None,
            bpf_flag: true,
            created_at: Utc::now().naive_utc(),
            status: "in_progress".to_string(),
            signer: Some("test_signer".to_string()),
            arch: None,
        };

        // Test insert
        let insert_result = client.insert_build_params(&build).await;
        assert!(insert_result.is_ok());
    }
}
