use super::DbClient;
use crate::{db::models::SolanaProgramBuild, errors::ApiError, Result};
use diesel_async::RunQueryDsl;
use tracing::{error, info};

impl DbClient {
    /// Insert a row in `solana_program_builds` for the build the caller is about to kick off.
    /// Used by `/verify*` so `/job/:id` can report status.
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
