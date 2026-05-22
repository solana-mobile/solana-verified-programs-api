//! Postgres data layer. Two tables of substance: `builds` (one row per
//! verification attempt — job + result merged) and `program_state` (one
//! cached row per program). All queries are compile-time-checked via
//! sqlx's `query!`/`query_as!`/`query_scalar!` macros.

use crate::{
    config::CONFIG, error::ApiError, error::Result, onchain::ProgramOnchainState, types::ProgramId,
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;
use tracing::info;
use uuid::Uuid;

pub const PER_PAGE: i64 = 20;
pub const BUILD_STATUS_IN_PROGRESS: &str = "in_progress";
pub const BUILD_STATUS_COMPLETED: &str = "completed";
pub const BUILD_STATUS_FAILED: &str = "failed";

#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Opens a bounded connection pool against `url`.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(CONFIG.db_max_connections)
            .acquire_timeout(Duration::from_secs(30))
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Runs all pending embedded migrations.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| ApiError::Internal(format!("migration: {e}")))?;
        info!("migrations applied");
        Ok(())
    }

    /// `SELECT 1` for the health endpoint.
    pub async fn ping(&self) -> Result<()> {
        sqlx::query!("SELECT 1 AS one")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRow {
    pub id: Uuid,
    pub repository: String,
    pub commit_hash: Option<String>,
    pub program_id: String,
    pub lib_name: Option<String>,
    pub base_docker_image: Option<String>,
    pub mount_path: Option<String>,
    pub cargo_args: Option<Vec<String>>,
    pub bpf_flag: bool,
    pub arch: Option<String>,
    pub signer: Option<String>,
    pub status: String,
    pub executable_hash: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Subset of `program_state` callers actually read. `authority` and
/// `last_checked` exist on the row but aren't surfaced anywhere yet.
#[derive(Debug, Clone)]
pub struct ProgramStateRow {
    pub on_chain_hash: Option<String>,
    pub is_frozen: bool,
    pub is_closed: bool,
}

/// Identifying parameters for a build, before insertion.
#[derive(Debug, Clone)]
pub struct NewBuild {
    pub repository: String,
    pub commit_hash: Option<String>,
    pub program_id: String,
    pub lib_name: Option<String>,
    pub base_docker_image: Option<String>,
    pub mount_path: Option<String>,
    pub cargo_args: Option<Vec<String>>,
    pub bpf_flag: bool,
    pub arch: Option<String>,
    pub signer: Option<String>,
}

impl Db {
    /// Inserts an `in_progress` build row and returns its UUID.
    pub async fn insert_build(&self, b: &NewBuild) -> Result<Uuid> {
        let id = Uuid::new_v4();
        sqlx::query!(
            "INSERT INTO builds (
                id, repository, commit_hash, program_id, lib_name,
                base_docker_image, mount_path, cargo_args, bpf_flag, arch,
                signer, status
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            id,
            b.repository,
            b.commit_hash,
            b.program_id,
            b.lib_name,
            b.base_docker_image,
            b.mount_path,
            b.cargo_args.as_deref(),
            b.bpf_flag,
            b.arch,
            b.signer,
            BUILD_STATUS_IN_PROGRESS,
        )
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Transitions a build to `completed` and records its executable hash.
    pub async fn mark_build_completed(&self, id: Uuid, executable_hash: &str) -> Result<()> {
        sqlx::query!(
            "UPDATE builds SET status = $1, executable_hash = $2, completed_at = NOW()
             WHERE id = $3",
            BUILD_STATUS_COMPLETED,
            executable_hash,
            id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Transitions a build to `failed` with the given error message.
    pub async fn mark_build_failed(&self, id: Uuid, error: &str) -> Result<()> {
        sqlx::query!(
            "UPDATE builds SET status = $1, error_message = $2, completed_at = NOW()
             WHERE id = $3",
            BUILD_STATUS_FAILED,
            error,
            id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetches a build by id.
    pub async fn get_build(&self, id: Uuid) -> Result<Option<BuildRow>> {
        Ok(
            sqlx::query_as!(BuildRow, "SELECT * FROM builds WHERE id = $1", id,)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Most recent non-failed build with identical params. Failed rows are
    /// ignored — they're retryable.
    pub async fn find_duplicate(&self, b: &NewBuild) -> Result<Option<BuildRow>> {
        Ok(sqlx::query_as!(
            BuildRow,
            "SELECT * FROM builds
             WHERE program_id = $1
               AND repository = $2
               AND (commit_hash IS NOT DISTINCT FROM $3)
               AND (lib_name IS NOT DISTINCT FROM $4)
               AND (base_docker_image IS NOT DISTINCT FROM $5)
               AND (mount_path IS NOT DISTINCT FROM $6)
               AND (cargo_args IS NOT DISTINCT FROM $7)
               AND bpf_flag = $8
               AND (arch IS NOT DISTINCT FROM $9)
               AND (signer IS NOT DISTINCT FROM $10)
               AND status <> 'failed'
             ORDER BY created_at DESC
             LIMIT 1",
            b.program_id,
            b.repository,
            b.commit_hash,
            b.lib_name,
            b.base_docker_image,
            b.mount_path,
            b.cargo_args.as_deref(),
            b.bpf_flag,
            b.arch,
            b.signer,
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    /// One row per signer — the signer's most recent completed claim for the program.
    pub async fn completed_builds_by_signer(
        &self,
        program_id: &ProgramId,
    ) -> Result<Vec<BuildRow>> {
        Ok(sqlx::query_as!(
            BuildRow,
            "SELECT DISTINCT ON (signer) *
             FROM builds
             WHERE program_id = $1 AND status = 'completed'
             ORDER BY signer, completed_at DESC",
            program_id.as_str(),
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Every completed build with this executable hash.
    pub async fn builds_by_executable_hash(&self, hash: &str) -> Result<Vec<BuildRow>> {
        Ok(sqlx::query_as!(
            BuildRow,
            "SELECT * FROM builds
             WHERE executable_hash = $1 AND status = 'completed'
             ORDER BY completed_at DESC",
            hash,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Most recent completed build for the program. When `prefer_hash` is
    /// set, prefers a build whose `executable_hash` matches; falls back to
    /// the latest of any hash. Well-known signers (the legacy DEFAULT_SIGNER
    /// + SIGNER_KEYS) win the tie-break so responses match the legacy API.
    pub async fn best_build(
        &self,
        program_id: &ProgramId,
        prefer_hash: Option<&str>,
    ) -> Result<Option<BuildRow>> {
        Ok(sqlx::query_as!(
            BuildRow,
            "SELECT * FROM builds
             WHERE program_id = $1 AND status = 'completed'
             ORDER BY (executable_hash IS NOT DISTINCT FROM $2) DESC,
                      CASE signer
                          WHEN '11111111111111111111111111111111' THEN 0
                          WHEN '9VWiUUhgNoRwTH5NVehYJEDwcotwYX3VgW4MChiHPAqU' THEN 1
                          WHEN 'CyJj5ejJAUveDXnLduJbkvwjxcmWJNqCuB9DR7AExrHn' THEN 1
                          WHEN '5vJwnLeyjV8uNJSp1zn7VLW8GwiQbcsQbGaVSwRmkE4r' THEN 1
                          ELSE 2
                      END ASC,
                      completed_at DESC
             LIMIT 1",
            program_id.as_str(),
            prefer_hash,
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Cached on-chain state for a program.
    pub async fn get_program_state(&self, program_id: &str) -> Result<Option<ProgramStateRow>> {
        Ok(sqlx::query_as!(
            ProgramStateRow,
            "SELECT on_chain_hash, is_frozen, is_closed FROM program_state WHERE program_id = $1",
            program_id,
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Full refresh from a snapshot. A `None` hash on the snapshot preserves
    /// the existing column rather than clobbering it, so a transient hash
    /// fetch failure doesn't lose previously known data.
    pub async fn upsert_program_state(
        &self,
        program_id: &str,
        state: &ProgramOnchainState,
    ) -> Result<()> {
        sqlx::query!(
            "INSERT INTO program_state
                (program_id, on_chain_hash, authority, is_frozen, is_closed, last_checked)
             VALUES ($1, $2, $3, $4, $5, NOW())
             ON CONFLICT (program_id) DO UPDATE
             SET on_chain_hash = COALESCE(EXCLUDED.on_chain_hash, program_state.on_chain_hash),
                 authority     = EXCLUDED.authority,
                 is_frozen     = EXCLUDED.is_frozen,
                 is_closed     = EXCLUDED.is_closed,
                 last_checked  = NOW()",
            program_id,
            state.executable_hash,
            state.authority,
            state.is_frozen,
            state.is_closed,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Records a program as closed and clears its authority.
    pub async fn mark_closed(&self, program_id: &str) -> Result<()> {
        sqlx::query!(
            "INSERT INTO program_state (program_id, is_closed, last_checked)
             VALUES ($1, TRUE, NOW())
             ON CONFLICT (program_id) DO UPDATE
             SET is_closed = TRUE, authority = NULL, last_checked = NOW()",
            program_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One page of currently-verified program IDs plus the total count.
    /// `search` (empty disables filtering) is matched against both
    /// `program_id` and `repository`.
    pub async fn verified_programs_page(
        &self,
        page: i64,
        search: &str,
    ) -> Result<(Vec<String>, i64)> {
        let page = page.max(1);
        let offset = (page - 1) * PER_PAGE;
        let pattern = format!("%{search}%");

        let total = sqlx::query_scalar!(
            r#"SELECT COUNT(DISTINCT b.program_id) AS "n!"
               FROM builds b
               LEFT JOIN program_state ps ON ps.program_id = b.program_id
               WHERE b.status = 'completed'
                 AND b.executable_hash IS NOT NULL
                 AND b.executable_hash = ps.on_chain_hash
                 AND NOT COALESCE(ps.is_closed, FALSE)
                 AND NOT COALESCE(ps.is_frozen, FALSE)
                 AND ($1 = '' OR b.program_id ILIKE $2 OR b.repository ILIKE $2)"#,
            search,
            pattern,
        )
        .fetch_one(&self.pool)
        .await?;

        let ids = sqlx::query_scalar!(
            "SELECT DISTINCT b.program_id
             FROM builds b
             LEFT JOIN program_state ps ON ps.program_id = b.program_id
             WHERE b.status = 'completed'
               AND b.executable_hash IS NOT NULL
               AND b.executable_hash = ps.on_chain_hash
               AND NOT COALESCE(ps.is_closed, FALSE)
               AND NOT COALESCE(ps.is_frozen, FALSE)
               AND ($1 = '' OR b.program_id ILIKE $2 OR b.repository ILIKE $2)
             ORDER BY b.program_id
             LIMIT $3 OFFSET $4",
            search,
            pattern,
            PER_PAGE,
            offset,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok((ids, total))
    }

    /// Latest completed build for every program that's currently verified
    /// (build hash matches on-chain hash) and not closed/frozen. The join
    /// guarantees `executable_hash == program_state.on_chain_hash`, so the
    /// state row carries no info the build doesn't already.
    pub async fn currently_verified_builds(&self) -> Result<Vec<BuildRow>> {
        Ok(sqlx::query_as!(
            BuildRow,
            "SELECT DISTINCT ON (b.program_id) b.*
             FROM builds b
             JOIN program_state ps ON ps.program_id = b.program_id
             WHERE b.status = 'completed'
               AND b.executable_hash IS NOT NULL
               AND b.executable_hash = ps.on_chain_hash
               AND NOT ps.is_closed
               AND NOT ps.is_frozen
             ORDER BY b.program_id, b.completed_at DESC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Every program ID the sweep should refresh: existing `program_state`
    /// rows, plus completed builds (so a program with a build but no state
    /// row yet — e.g. after a dropped webhook — gets bootstrapped).
    /// Ordered oldest-first so a partial cycle still drains the staleness.
    pub async fn sweep_program_ids(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar!(
            r#"
            SELECT program_id AS "program_id!" FROM (
                SELECT ps.program_id, ps.last_checked
                FROM program_state ps
                UNION
                SELECT b.program_id, NULL::timestamptz AS last_checked
                FROM (SELECT DISTINCT program_id FROM builds WHERE status = 'completed') b
                WHERE NOT EXISTS (SELECT 1 FROM program_state ps WHERE ps.program_id = b.program_id)
            ) q
            ORDER BY last_checked ASC NULLS FIRST
            "#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Proxy for "is the sweep still running" — used by the health endpoints.
    pub async fn last_sweep_at(&self) -> Result<Option<DateTime<Utc>>> {
        Ok(
            sqlx::query_scalar!(r#"SELECT MAX(last_checked) AS "max" FROM program_state"#)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Records the on-disk log filename for a failed build.
    pub async fn insert_build_log(
        &self,
        build_id: Uuid,
        program_id: &str,
        file_name: &str,
    ) -> Result<()> {
        sqlx::query!(
            "INSERT INTO build_logs (id, program_id, file_name) VALUES ($1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET file_name = EXCLUDED.file_name",
            build_id,
            program_id,
            file_name,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Looks up the on-disk log filename for a build.
    pub async fn get_build_log_file(&self, build_id: Uuid) -> Result<Option<String>> {
        Ok(sqlx::query_scalar!(
            "SELECT file_name FROM build_logs
             WHERE id = $1
             ORDER BY created_at DESC
             LIMIT 1",
            build_id,
        )
        .fetch_optional(&self.pool)
        .await?)
    }
}
