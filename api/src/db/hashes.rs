use super::DbClient;
use crate::db::models::{BuildArgs, ResolveHashResponse, VerifiedHash};
use crate::Result;
use diesel::{
    expression_methods::ExpressionMethods, query_dsl::QueryDsl, OptionalExtension,
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

