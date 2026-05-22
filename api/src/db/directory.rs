//! Content-addressed view of verified builds. This isn't its own table —
//! it's a query shape over `verified_programs` joined to `solana_program_builds`.
//! A "directory entry" is the result row of that join filtered by
//! `vp.is_verified = true`.

use super::DbClient;
use crate::db::models::{BuildArgs, ResolveHashResponse};
use crate::Result;
use chrono::NaiveDateTime;
use diesel::{sql_query, sql_types, QueryableByName};
use diesel_async::RunQueryDsl;
use tracing::error;

#[derive(QueryableByName)]
struct DirectoryRow {
    #[diesel(sql_type = sql_types::Text)]
    executable_hash: String,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
    signer: Option<String>,
    #[diesel(sql_type = sql_types::Text)]
    repository: String,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
    commit_hash: Option<String>,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
    lib_name: Option<String>,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
    base_docker_image: Option<String>,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
    mount_path: Option<String>,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Array<sql_types::Text>>)]
    cargo_args: Option<Vec<String>>,
    #[diesel(sql_type = sql_types::Bool)]
    bpf_flag: bool,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
    arch: Option<String>,
    #[diesel(sql_type = sql_types::Timestamp)]
    verified_at: NaiveDateTime,
}

impl From<DirectoryRow> for ResolveHashResponse {
    fn from(r: DirectoryRow) -> Self {
        Self {
            executable_hash: r.executable_hash,
            signer: r.signer.unwrap_or_default(),
            repository: r.repository,
            commit: r.commit_hash,
            build_args: BuildArgs {
                lib_name: r.lib_name,
                base_image: r.base_docker_image,
                mount_path: r.mount_path,
                cargo_args: r.cargo_args,
                bpf_flag: r.bpf_flag,
                arch: r.arch,
            },
            verified_at: r.verified_at,
        }
    }
}

impl DbClient {
    /// Every signer's verified-build claim about `executable_hash`. The shape
    /// is one row per signer; multiple rows means multiple signers have each
    /// independently published a build whose bytes hash to the same value.
    /// Returns deduplicated rows in `verified_at DESC` order — if the same
    /// `(executable_hash, signer)` appears multiple times across builds, the
    /// most recent verification wins.
    pub async fn get_verified_builds_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<ResolveHashResponse>> {
        let conn = &mut self.get_db_conn().await?;
        let rows: Vec<DirectoryRow> = sql_query(
            r#"
            SELECT DISTINCT ON (vp.executable_hash, sp.signer)
                vp.executable_hash,
                sp.signer,
                sp.repository,
                sp.commit_hash,
                sp.lib_name,
                sp.base_docker_image,
                sp.mount_path,
                sp.cargo_args,
                sp.bpf_flag,
                sp.arch,
                vp.verified_at
            FROM verified_programs vp
            JOIN solana_program_builds sp ON sp.id = vp.solana_build_id
            WHERE vp.is_verified = true
              AND vp.executable_hash = $1
            ORDER BY vp.executable_hash, sp.signer, vp.verified_at DESC
            "#,
        )
        .bind::<sql_types::Text, _>(hash)
        .load(conn)
        .await
        .map_err(|e| {
            error!("Failed to load verified builds for hash {}: {}", hash, e);
            e
        })?;

        Ok(rows.into_iter().map(Into::into).collect())
    }
}
