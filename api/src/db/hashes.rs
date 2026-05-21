use super::DbClient;
use crate::db::models::{
    BuildArgs, ResolveHashResponse, SolanaProgramBuildParams, VerifiedHash,
};
use crate::Result;
use diesel::{
    expression_methods::ExpressionMethods, query_dsl::QueryDsl, sql_query, OptionalExtension,
};
use diesel_async::RunQueryDsl;
use tracing::error;

/// DbClient helper functions for the content-addressed `verified_hashes` directory.
impl DbClient {
    /// Insert (or upsert) a verified-build claim.
    pub async fn insert_or_update_verified_hash(&self, entry: &VerifiedHash) -> Result<usize> {
        use crate::schema::verified_hashes::dsl::*;

        let conn = &mut self.get_db_conn().await?;

        diesel::insert_into(verified_hashes)
            .values(entry)
            .on_conflict((executable_hash, signer))
            .do_update()
            .set(entry)
            .execute(conn)
            .await
            .map_err(|e| {
                error!("Failed to upsert verified_hash: {}", e);
                e.into()
            })
    }

    /// Look up a directory row matching the given build params, regardless of
    /// which signer claimed it. Used by `/verify*` to short-circuit a rebuild
    /// when the bytes for that config are already known.
    pub async fn find_hash_for_build_params(
        &self,
        params: &SolanaProgramBuildParams,
    ) -> Result<Option<VerifiedHash>> {
        let conn = &mut self.get_db_conn().await?;
        sql_query(
            r#"
            SELECT * FROM verified_hashes
            WHERE repository                  = $1
              AND commit_hash       IS NOT DISTINCT FROM $2
              AND lib_name          IS NOT DISTINCT FROM $3
              AND base_docker_image IS NOT DISTINCT FROM $4
              AND mount_path        IS NOT DISTINCT FROM $5
              AND cargo_args        IS NOT DISTINCT FROM $6
              AND bpf_flag                       = $7
              AND arch              IS NOT DISTINCT FROM $8
            ORDER BY verified_at DESC
            LIMIT 1
            "#,
        )
        .bind::<diesel::sql_types::Text, _>(&params.repository)
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&params.commit_hash)
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&params.lib_name)
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&params.base_image)
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&params.mount_path)
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Array<diesel::sql_types::Text>>, _>(
            &params.cargo_args,
        )
        .bind::<diesel::sql_types::Bool, _>(params.bpf_flag.unwrap_or(false))
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&params.arch)
        .get_result::<VerifiedHash>(conn)
        .await
        .optional()
        .map_err(|e| {
            error!("Failed to query verified_hashes by build params: {}", e);
            e.into()
        })
    }

    /// All directory rows for a given `executable_hash`. Used by `/resolve-hash`.
    pub async fn get_verified_hashes_by_hash(&self, hash: &str) -> Result<Vec<VerifiedHash>> {
        use crate::schema::verified_hashes::dsl::*;
        let conn = &mut self.get_db_conn().await?;
        verified_hashes
            .filter(executable_hash.eq(hash))
            .load::<VerifiedHash>(conn)
            .await
            .map_err(|e| {
                error!("Failed to load verified_hashes for {}: {}", hash, e);
                e.into()
            })
    }

    /// Directory rows for `executable_hash` whose signer is in `trust_set`.
    /// Result is sorted to match the caller's trust ordering.
    pub async fn get_verified_hashes_trusted(
        &self,
        hash: &str,
        trust_set: &[String],
    ) -> Result<Vec<VerifiedHash>> {
        if trust_set.is_empty() {
            return Ok(vec![]);
        }
        use crate::schema::verified_hashes::dsl::*;
        let conn = &mut self.get_db_conn().await?;
        let rows: Vec<VerifiedHash> = verified_hashes
            .filter(executable_hash.eq(hash))
            .filter(signer.eq_any(trust_set))
            .load::<VerifiedHash>(conn)
            .await
            .map_err(|e| {
                error!("Failed to load trusted verified_hashes for {}: {}", hash, e);
                e
            })?;
        let mut sorted = rows;
        sorted.sort_by_key(|r| {
            trust_set
                .iter()
                .position(|s| s == &r.signer)
                .unwrap_or(usize::MAX)
        });
        Ok(sorted)
    }
}

impl From<VerifiedHash> for ResolveHashResponse {
    fn from(v: VerifiedHash) -> Self {
        ResolveHashResponse {
            executable_hash: v.executable_hash,
            signer: v.signer,
            repository: v.repository,
            commit: v.commit_hash,
            build_args: BuildArgs {
                lib_name: v.lib_name,
                base_image: v.base_docker_image,
                mount_path: v.mount_path,
                cargo_args: v.cargo_args,
                bpf_flag: v.bpf_flag,
                arch: v.arch,
            },
            verified_at: v.verified_at,
        }
    }
}
