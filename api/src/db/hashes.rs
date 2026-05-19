use super::DbClient;
use crate::db::models::{BuildArgs, ResolveHashResponse, SolanaProgramBuildParams, VerifiedHash};
use crate::Result;
use diesel::{
    expression_methods::ExpressionMethods, query_dsl::QueryDsl, sql_query, OptionalExtension,
};
use diesel_async::RunQueryDsl;
use tracing::error;

impl DbClient {
    /// Insert (or upsert) a content-addressed verified build entry.
    /// A row exists iff `(repository, commit_hash, build_args)` deterministically
    /// produces `executable_hash`.
    pub async fn insert_or_update_verified_hash(&self, entry: &VerifiedHash) -> Result<usize> {
        use crate::schema::verified_hashes::dsl::*;

        let conn = &mut self.get_db_conn().await?;

        diesel::insert_into(verified_hashes)
            .values(entry)
            .on_conflict(executable_hash)
            .do_update()
            .set(entry)
            .execute(conn)
            .await
            .map_err(|e| {
                error!("Failed to upsert verified_hash: {}", e);
                e.into()
            })
    }

    /// Look up a directory entry that matches the given build configuration.
    /// Used by the `/verify*` endpoints to fast-path on a cache hit: if some
    /// `(repository, commit, build_args)` has already produced a hash, skip
    /// the build and return that hash.
    pub async fn find_hash_for_build_params(
        &self,
        params: &SolanaProgramBuildParams,
    ) -> Result<Option<VerifiedHash>> {
        let conn = &mut self.get_db_conn().await?;
        // `IS NOT DISTINCT FROM` is null-safe equality — matches NULL=NULL too.
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

    /// Look up a verified build by its executable hash.
    /// Returns `Ok(None)` when no row matches.
    pub async fn get_verified_hash(&self, hash: &str) -> Result<Option<VerifiedHash>> {
        use crate::schema::verified_hashes::dsl::*;

        let conn = &mut self.get_db_conn().await?;

        let row: Option<VerifiedHash> = verified_hashes
            .filter(executable_hash.eq(hash))
            .first::<VerifiedHash>(conn)
            .await
            .optional()
            .map_err(|e| {
                error!("Failed to fetch verified_hash {}: {}", hash, e);
                e
            })?;

        Ok(row)
    }

}

impl From<VerifiedHash> for ResolveHashResponse {
    fn from(v: VerifiedHash) -> Self {
        ResolveHashResponse {
            executable_hash: v.executable_hash,
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

