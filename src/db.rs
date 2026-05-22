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

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProgramStateRow {
    pub program_id: String,
    pub on_chain_hash: Option<String>,
    pub authority: Option<String>,
    pub is_frozen: bool,
    pub is_closed: bool,
    pub last_checked: DateTime<Utc>,
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
        Ok(sqlx::query_as!(
            BuildRow,
            "SELECT * FROM builds WHERE id = $1",
            id,
        )
        .fetch_optional(&self.pool)
        .await?)
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

    /// Both halves of a `/status` response in one round trip: the cached
    /// chain state plus the best matching build. Tie-break: prefer the build
    /// whose `executable_hash` matches the on-chain hash, then prefer
    /// well-known signers (matches the legacy API's selection), then prefer
    /// most-recently completed.
    pub async fn status_row(
        &self,
        program_id: &ProgramId,
    ) -> Result<Option<(ProgramStateRow, Option<BuildRow>)>> {
        let row = sqlx::query!(
            r#"
            SELECT
                ps.program_id    AS "ps_program_id!",
                ps.on_chain_hash AS "ps_on_chain_hash",
                ps.authority     AS "ps_authority",
                ps.is_frozen     AS "ps_is_frozen!",
                ps.is_closed     AS "ps_is_closed!",
                ps.last_checked  AS "ps_last_checked!",
                b.id                AS "b_id?",
                b.repository        AS "b_repository?",
                b.commit_hash       AS "b_commit_hash?",
                b.program_id        AS "b_program_id?",
                b.lib_name          AS "b_lib_name?",
                b.base_docker_image AS "b_base_docker_image?",
                b.mount_path        AS "b_mount_path?",
                b.cargo_args        AS "b_cargo_args?: Vec<String>",
                b.bpf_flag          AS "b_bpf_flag?",
                b.arch              AS "b_arch?",
                b.signer            AS "b_signer?",
                b.status            AS "b_status?",
                b.executable_hash   AS "b_executable_hash?",
                b.error_message     AS "b_error_message?",
                b.created_at        AS "b_created_at?",
                b.completed_at      AS "b_completed_at?"
            FROM program_state ps
            LEFT JOIN LATERAL (
                SELECT *
                FROM builds
                WHERE program_id = ps.program_id AND status = 'completed'
                ORDER BY (executable_hash IS NOT DISTINCT FROM ps.on_chain_hash) DESC,
                         CASE signer
                             WHEN '11111111111111111111111111111111' THEN 0
                             WHEN '9VWiUUhgNoRwTH5NVehYJEDwcotwYX3VgW4MChiHPAqU' THEN 1
                             WHEN 'CyJj5ejJAUveDXnLduJbkvwjxcmWJNqCuB9DR7AExrHn' THEN 1
                             WHEN '5vJwnLeyjV8uNJSp1zn7VLW8GwiQbcsQbGaVSwRmkE4r' THEN 1
                             ELSE 2
                         END ASC,
                         completed_at DESC
                LIMIT 1
            ) b ON TRUE
            WHERE ps.program_id = $1
            "#,
            program_id.as_str(),
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let state = ProgramStateRow {
                program_id: r.ps_program_id,
                on_chain_hash: r.ps_on_chain_hash,
                authority: r.ps_authority,
                is_frozen: r.ps_is_frozen,
                is_closed: r.ps_is_closed,
                last_checked: r.ps_last_checked,
            };
            let build = r.b_id.map(|id| BuildRow {
                id,
                repository: r.b_repository.unwrap_or_default(),
                commit_hash: r.b_commit_hash,
                program_id: r.b_program_id.unwrap_or_default(),
                lib_name: r.b_lib_name,
                base_docker_image: r.b_base_docker_image,
                mount_path: r.b_mount_path,
                cargo_args: r.b_cargo_args,
                bpf_flag: r.b_bpf_flag.unwrap_or(false),
                arch: r.b_arch,
                signer: r.b_signer,
                status: r.b_status.unwrap_or_default(),
                executable_hash: r.b_executable_hash,
                error_message: r.b_error_message,
                created_at: r.b_created_at.unwrap_or_else(Utc::now),
                completed_at: r.b_completed_at,
            });
            (state, build)
        }))
    }

    /// Cached on-chain state for a program.
    pub async fn get_program_state(&self, program_id: &str) -> Result<Option<ProgramStateRow>> {
        Ok(sqlx::query_as!(
            ProgramStateRow,
            "SELECT * FROM program_state WHERE program_id = $1",
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

    /// Latest completed build paired with its state, for every program
    /// currently verified and not closed/frozen.
    pub async fn verified_programs_with_state(&self) -> Result<Vec<(BuildRow, ProgramStateRow)>> {
        let rows = sqlx::query!(
            r#"
            SELECT
                b.id                AS "b_id!",
                b.repository        AS "b_repository!",
                b.commit_hash       AS "b_commit_hash",
                b.program_id        AS "b_program_id!",
                b.lib_name          AS "b_lib_name",
                b.base_docker_image AS "b_base_docker_image",
                b.mount_path        AS "b_mount_path",
                b.cargo_args        AS "b_cargo_args: Vec<String>",
                b.bpf_flag          AS "b_bpf_flag!",
                b.arch              AS "b_arch",
                b.signer            AS "b_signer",
                b.status            AS "b_status!",
                b.executable_hash   AS "b_executable_hash",
                b.error_message     AS "b_error_message",
                b.created_at        AS "b_created_at!",
                b.completed_at      AS "b_completed_at",
                ps.program_id    AS "ps_program_id!",
                ps.on_chain_hash AS "ps_on_chain_hash",
                ps.authority     AS "ps_authority",
                ps.is_frozen     AS "ps_is_frozen!",
                ps.is_closed     AS "ps_is_closed!",
                ps.last_checked  AS "ps_last_checked!"
            FROM (
                SELECT DISTINCT ON (program_id) *
                FROM builds
                WHERE status = 'completed' AND executable_hash IS NOT NULL
                ORDER BY program_id, completed_at DESC
            ) b
            JOIN program_state ps ON ps.program_id = b.program_id
            WHERE b.executable_hash = ps.on_chain_hash
              AND NOT ps.is_closed
              AND NOT ps.is_frozen
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let build = BuildRow {
                    id: r.b_id,
                    repository: r.b_repository,
                    commit_hash: r.b_commit_hash,
                    program_id: r.b_program_id,
                    lib_name: r.b_lib_name,
                    base_docker_image: r.b_base_docker_image,
                    mount_path: r.b_mount_path,
                    cargo_args: r.b_cargo_args,
                    bpf_flag: r.b_bpf_flag,
                    arch: r.b_arch,
                    signer: r.b_signer,
                    status: r.b_status,
                    executable_hash: r.b_executable_hash,
                    error_message: r.b_error_message,
                    created_at: r.b_created_at,
                    completed_at: r.b_completed_at,
                };
                let state = ProgramStateRow {
                    program_id: r.ps_program_id,
                    on_chain_hash: r.ps_on_chain_hash,
                    authority: r.ps_authority,
                    is_frozen: r.ps_is_frozen,
                    is_closed: r.ps_is_closed,
                    last_checked: r.ps_last_checked,
                };
                (build, state)
            })
            .collect())
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
