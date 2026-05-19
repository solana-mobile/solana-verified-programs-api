use super::DbClient;
use crate::db::models::{BuildArgs, ResolveHashResponse, VerifiedHash};
use crate::Result;
use diesel::{
    expression_methods::ExpressionMethods, query_dsl::QueryDsl, OptionalExtension,
};
use diesel_async::RunQueryDsl;
use tracing::{error, info};

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

    /// Find a cached entry that matches the given build configuration.
    /// Used by `POST /compute-hash` to avoid re-running a build when the same
    /// `(repository, commit, build_args)` has already produced a hash.
    #[allow(clippy::too_many_arguments)]
    pub async fn find_verified_hash_by_config(
        &self,
        repository_value: &str,
        commit_hash_value: Option<&str>,
        lib_name_value: Option<&str>,
        base_image_value: Option<&str>,
        mount_path_value: Option<&str>,
        cargo_args_value: Option<&[String]>,
        bpf_flag_value: bool,
        arch_value: Option<&str>,
    ) -> Result<Option<VerifiedHash>> {
        use crate::schema::verified_hashes::dsl::*;

        let conn = &mut self.get_db_conn().await?;

        let mut query = verified_hashes
            .filter(repository.eq(repository_value))
            .filter(bpf_flag.eq(bpf_flag_value))
            .into_boxed();

        if let Some(v) = commit_hash_value {
            query = query.filter(commit_hash.eq(v));
        } else {
            query = query.filter(commit_hash.is_null());
        }
        if let Some(v) = lib_name_value {
            query = query.filter(lib_name.eq(v));
        } else {
            query = query.filter(lib_name.is_null());
        }
        if let Some(v) = base_image_value {
            query = query.filter(base_docker_image.eq(v));
        } else {
            query = query.filter(base_docker_image.is_null());
        }
        if let Some(v) = mount_path_value {
            query = query.filter(mount_path.eq(v));
        } else {
            query = query.filter(mount_path.is_null());
        }
        if let Some(v) = cargo_args_value {
            query = query.filter(cargo_args.eq(v.to_vec()));
        } else {
            query = query.filter(cargo_args.is_null());
        }
        if let Some(v) = arch_value {
            query = query.filter(arch.eq(v));
        } else {
            query = query.filter(arch.is_null());
        }

        let row: Option<VerifiedHash> = query
            .order(verified_at.desc())
            .first::<VerifiedHash>(conn)
            .await
            .optional()
            .map_err(|e| {
                error!("Failed to query verified_hashes by config: {}", e);
                e
            })?;

        info!(
            "find_verified_hash_by_config repository={} commit={:?} lib={:?} -> {}",
            repository_value,
            commit_hash_value,
            lib_name_value,
            if row.is_some() { "hit" } else { "miss" }
        );

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

