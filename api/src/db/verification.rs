use super::DbClient;
use crate::db::models::{VerifiedProgram, DEFAULT_SIGNER};
use crate::services::onchain::program_metadata_retriever::SIGNER_KEYS;
use crate::Result;
use diesel::{
    expression_methods::{BoolExpressionMethods, ExpressionMethods},
    query_dsl::QueryDsl,
    Table,
};
use diesel_async::RunQueryDsl;

use tracing::{error, info};

/// DbClient helper functions for VerifiedPrograms table.
impl DbClient {
    /// Get the verification status for a program
    ///
    /// Returns a VerifiedProgram struct
    pub async fn get_verified_build(
        &self,
        program_address: &str,
        signer: Option<String>,
    ) -> Result<VerifiedProgram> {
        use crate::schema::verified_programs::dsl::*;

        info!("Getting verified build for {:?}", program_address);
        let conn = &mut self.get_db_conn().await?;

        let query = verified_programs
            .left_join(crate::schema::solana_program_builds::table)
            .filter(program_id.eq(program_address))
            .select(verified_programs::all_columns())
            .order((is_verified.desc(), verified_at.desc()));

        match signer {
            Some(signer) => query
                .filter(crate::schema::solana_program_builds::signer.eq(signer))
                .first::<VerifiedProgram>(conn)
                .await
                .map_err(|e| {
                    error!("Failed to get solana_program_builds: {}", e);
                    e.into()
                }),
            None => {
                let program_authority = self.get_program_authority_from_db(program_address).await;
                let mut filtered_query = query
                    .filter(
                        crate::schema::solana_program_builds::signer
                            .eq(Some(DEFAULT_SIGNER.to_string()))
                            .or(crate::schema::solana_program_builds::signer
                                .eq(Some(SIGNER_KEYS[0].to_string())))
                            .or(crate::schema::solana_program_builds::signer
                                .eq(Some(SIGNER_KEYS[1].to_string())))
                            .or(crate::schema::solana_program_builds::signer
                                .eq(Some(SIGNER_KEYS[2].to_string())))
                            .or(crate::schema::solana_program_builds::signer.is_null()),
                    )
                    .into_boxed();

                if let Ok(Some(program_authority)) = program_authority {
                    filtered_query = query
                        .filter(
                            crate::schema::solana_program_builds::signer
                                .eq(Some(DEFAULT_SIGNER.to_string()))
                                .or(crate::schema::solana_program_builds::signer
                                    .eq(Some(SIGNER_KEYS[0].to_string())))
                                .or(crate::schema::solana_program_builds::signer
                                    .eq(Some(SIGNER_KEYS[1].to_string())))
                                .or(crate::schema::solana_program_builds::signer
                                    .eq(Some(SIGNER_KEYS[2].to_string())))
                                .or(crate::schema::solana_program_builds::signer
                                    .eq(Some(program_authority)))
                                .or(crate::schema::solana_program_builds::signer.is_null()),
                        )
                        .into_boxed();
                }

                filtered_query
                    .first::<VerifiedProgram>(conn)
                    .await
                    .map_err(|e| {
                        error!("Failed to get verified program data: {}", e);
                        e.into()
                    })
            }
        }
    }

    /// Insert or update a verified program
    pub async fn insert_or_update_verified_build(
        &self,
        payload: &VerifiedProgram,
    ) -> Result<usize> {
        use crate::schema::verified_programs::dsl::*;

        let conn = &mut self.get_db_conn().await?;
        diesel::insert_into(verified_programs)
            .values(payload)
            .on_conflict(id)
            .do_update()
            .set(payload)
            .execute(conn)
            .await
            .map_err(|e| {
                error!("Failed to insert/update verified build: {}", e);
                e.into()
            })
    }


    /// Mark a program as unverified without modifying the on-chain hash.
    pub async fn mark_program_unverified(&self, program_address: &str) -> Result<usize> {
        use crate::schema::verified_programs::dsl::*;

        let conn = &mut self.get_db_conn().await?;

        diesel::update(verified_programs)
            .filter(program_id.eq(program_address))
            .set((
                is_verified.eq(false),
                verified_at.eq(chrono::Utc::now().naive_utc()),
            ))
            .execute(conn)
            .await
            .map_err(|e| {
                error!("Failed to mark program as unverified: {}", e);
                e.into()
            })
    }
}

