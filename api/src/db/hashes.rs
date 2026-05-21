use super::DbClient;
use crate::db::models::{
    BuildArgs, ResolveHashResponse, SolanaProgramBuildParams, VerifiedHash,
};
use crate::Result;
use diesel::{
    expression_methods::ExpressionMethods, query_dsl::QueryDsl, sql_query, OptionalExtension,
    QueryableByName,
};
use diesel_async::RunQueryDsl;
use tracing::error;

/// Page size for the paginated `/verified-programs/:page` view over the directory.
pub const PER_PAGE: i64 = 20;

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[derive(QueryableByName)]
struct ProgramIdRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    program_id: String,
}

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

    /// Paginated list of distinct program ids that have at least one
    /// completed build whose params join a row in the directory. Optional
    /// `search` term filters by `program_id` or `repository` (ILIKE).
    /// Returns `(page_ids, total_count)`.
    pub async fn list_verified_program_ids(
        &self,
        page: i64,
        search: Option<&str>,
    ) -> Result<(Vec<String>, i64)> {
        let conn = &mut self.get_db_conn().await?;
        let offset = (page.max(1) - 1) * PER_PAGE;
        let search = search.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
        let pattern = format!("%{}%", search);

        let total: i64 = sql_query(
            r#"
            SELECT COUNT(*)::bigint AS count FROM (
                SELECT DISTINCT sp.program_id
                FROM solana_program_builds sp
                WHERE sp.status = 'completed'
                  AND ($1 = '' OR sp.program_id ILIKE $2 OR sp.repository ILIKE $2)
                  AND EXISTS (
                    SELECT 1 FROM verified_hashes vh
                    WHERE vh.repository                  = sp.repository
                      AND vh.commit_hash       IS NOT DISTINCT FROM sp.commit_hash
                      AND vh.lib_name          IS NOT DISTINCT FROM sp.lib_name
                      AND vh.base_docker_image IS NOT DISTINCT FROM sp.base_docker_image
                      AND vh.mount_path        IS NOT DISTINCT FROM sp.mount_path
                      AND vh.cargo_args        IS NOT DISTINCT FROM sp.cargo_args
                      AND vh.bpf_flag                       = sp.bpf_flag
                      AND vh.arch              IS NOT DISTINCT FROM sp.arch
                  )
            ) t
            "#,
        )
        .bind::<diesel::sql_types::Text, _>(search)
        .bind::<diesel::sql_types::Text, _>(&pattern)
        .get_result::<CountRow>(conn)
        .await
        .map(|r| r.count)
        .unwrap_or(0);

        let rows: Vec<ProgramIdRow> = sql_query(
            r#"
            SELECT DISTINCT sp.program_id
            FROM solana_program_builds sp
            WHERE sp.status = 'completed'
              AND ($1 = '' OR sp.program_id ILIKE $2 OR sp.repository ILIKE $2)
              AND EXISTS (
                SELECT 1 FROM verified_hashes vh
                WHERE vh.repository                  = sp.repository
                  AND vh.commit_hash       IS NOT DISTINCT FROM sp.commit_hash
                  AND vh.lib_name          IS NOT DISTINCT FROM sp.lib_name
                  AND vh.base_docker_image IS NOT DISTINCT FROM sp.base_docker_image
                  AND vh.mount_path        IS NOT DISTINCT FROM sp.mount_path
                  AND vh.cargo_args        IS NOT DISTINCT FROM sp.cargo_args
                  AND vh.bpf_flag                       = sp.bpf_flag
                  AND vh.arch              IS NOT DISTINCT FROM sp.arch
              )
            ORDER BY sp.program_id
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind::<diesel::sql_types::Text, _>(search)
        .bind::<diesel::sql_types::Text, _>(&pattern)
        .bind::<diesel::sql_types::BigInt, _>(PER_PAGE)
        .bind::<diesel::sql_types::BigInt, _>(offset)
        .load(conn)
        .await
        .map_err(|e| {
            error!("Failed to load verified program ids: {}", e);
            e
        })?;

        Ok((rows.into_iter().map(|r| r.program_id).collect(), total))
    }

    /// Every program id with at least one completed build that joins a
    /// directory row. Used by `/verified-programs-status` (which iterates
    /// per-program). No pagination.
    pub async fn all_verified_program_ids(&self) -> Result<Vec<String>> {
        let conn = &mut self.get_db_conn().await?;
        let rows: Vec<ProgramIdRow> = sql_query(
            r#"
            SELECT DISTINCT sp.program_id
            FROM solana_program_builds sp
            WHERE sp.status = 'completed'
              AND EXISTS (
                SELECT 1 FROM verified_hashes vh
                WHERE vh.repository                  = sp.repository
                  AND vh.commit_hash       IS NOT DISTINCT FROM sp.commit_hash
                  AND vh.lib_name          IS NOT DISTINCT FROM sp.lib_name
                  AND vh.base_docker_image IS NOT DISTINCT FROM sp.base_docker_image
                  AND vh.mount_path        IS NOT DISTINCT FROM sp.mount_path
                  AND vh.cargo_args        IS NOT DISTINCT FROM sp.cargo_args
                  AND vh.bpf_flag                       = sp.bpf_flag
                  AND vh.arch              IS NOT DISTINCT FROM sp.arch
              )
            ORDER BY sp.program_id
            "#,
        )
        .load(conn)
        .await
        .map_err(|e| {
            error!("Failed to load all verified program ids: {}", e);
            e
        })?;
        Ok(rows.into_iter().map(|r| r.program_id).collect())
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
