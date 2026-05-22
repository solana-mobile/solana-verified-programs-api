//! Postgres data layer. Two tables of substance: `builds` (one row per
//! verification attempt — job + result merged) and `program_state` (one
//! cached row per program). Reads never call out to the chain.

use crate::{
    config::CONFIG, error::ApiError, error::Result, onchain::ProgramOnchainState, types::ProgramId,
};
use chrono::{DateTime, Utc};
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    PgPool, Row,
};
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
        sqlx::query("SELECT 1").execute(&self.pool).await?;
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

impl BuildRow {
    fn from_row(row: &PgRow) -> Self {
        BuildRow {
            id: row.get("id"),
            repository: row.get("repository"),
            commit_hash: row.get("commit_hash"),
            program_id: row.get("program_id"),
            lib_name: row.get("lib_name"),
            base_docker_image: row.get("base_docker_image"),
            mount_path: row.get("mount_path"),
            cargo_args: row.get("cargo_args"),
            bpf_flag: row.get("bpf_flag"),
            arch: row.get("arch"),
            signer: row.get("signer"),
            status: row.get("status"),
            executable_hash: row.get("executable_hash"),
            error_message: row.get("error_message"),
            created_at: row.get("created_at"),
            completed_at: row.get("completed_at"),
        }
    }
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

impl ProgramStateRow {
    fn from_row(row: &PgRow) -> Self {
        ProgramStateRow {
            program_id: row.get("program_id"),
            on_chain_hash: row.get("on_chain_hash"),
            authority: row.get("authority"),
            is_frozen: row.get("is_frozen"),
            is_closed: row.get("is_closed"),
            last_checked: row.get("last_checked"),
        }
    }
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
        sqlx::query(
            "INSERT INTO builds (
                id, repository, commit_hash, program_id, lib_name,
                base_docker_image, mount_path, cargo_args, bpf_flag, arch,
                signer, status
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(id)
        .bind(&b.repository)
        .bind(&b.commit_hash)
        .bind(&b.program_id)
        .bind(&b.lib_name)
        .bind(&b.base_docker_image)
        .bind(&b.mount_path)
        .bind(&b.cargo_args)
        .bind(b.bpf_flag)
        .bind(&b.arch)
        .bind(&b.signer)
        .bind(BUILD_STATUS_IN_PROGRESS)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Transitions a build to `completed` and records its executable hash.
    pub async fn mark_build_completed(&self, id: Uuid, executable_hash: &str) -> Result<()> {
        sqlx::query(
            "UPDATE builds SET status = $1, executable_hash = $2, completed_at = NOW()
             WHERE id = $3",
        )
        .bind(BUILD_STATUS_COMPLETED)
        .bind(executable_hash)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Transitions a build to `failed` with the given error message.
    pub async fn mark_build_failed(&self, id: Uuid, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE builds SET status = $1, error_message = $2, completed_at = NOW()
             WHERE id = $3",
        )
        .bind(BUILD_STATUS_FAILED)
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetches a build by id.
    pub async fn get_build(&self, id: Uuid) -> Result<Option<BuildRow>> {
        let row = sqlx::query("SELECT * FROM builds WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(BuildRow::from_row))
    }

    /// Most recent non-failed build with identical params. Failed rows are
    /// ignored — they're retryable.
    pub async fn find_duplicate(&self, b: &NewBuild) -> Result<Option<BuildRow>> {
        let row = sqlx::query(
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
        )
        .bind(&b.program_id)
        .bind(&b.repository)
        .bind(&b.commit_hash)
        .bind(&b.lib_name)
        .bind(&b.base_docker_image)
        .bind(&b.mount_path)
        .bind(&b.cargo_args)
        .bind(b.bpf_flag)
        .bind(&b.arch)
        .bind(&b.signer)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(BuildRow::from_row))
    }

    /// One row per signer — the signer's most recent completed claim for the program.
    pub async fn completed_builds_by_signer(
        &self,
        program_id: &ProgramId,
    ) -> Result<Vec<BuildRow>> {
        let rows = sqlx::query(
            "SELECT DISTINCT ON (signer) *
             FROM builds
             WHERE program_id = $1 AND status = 'completed'
             ORDER BY signer, completed_at DESC",
        )
        .bind(program_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(BuildRow::from_row).collect())
    }

    /// Every completed build with this executable hash.
    pub async fn builds_by_executable_hash(&self, hash: &str) -> Result<Vec<BuildRow>> {
        let rows = sqlx::query(
            "SELECT * FROM builds
             WHERE executable_hash = $1 AND status = 'completed'
             ORDER BY completed_at DESC",
        )
        .bind(hash)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(BuildRow::from_row).collect())
    }

    /// Both halves of a `/status` response in one round trip: the cached
    /// chain state plus the best matching build (same tie-break as
    /// [`Db::latest_completed_build`]).
    pub async fn status_row(
        &self,
        program_id: &ProgramId,
    ) -> Result<Option<(ProgramStateRow, Option<BuildRow>)>> {
        let pid = program_id.as_str();
        let row = sqlx::query(
            "SELECT
                ps.program_id    AS ps_program_id,
                ps.on_chain_hash AS ps_on_chain_hash,
                ps.authority     AS ps_authority,
                ps.is_frozen     AS ps_is_frozen,
                ps.is_closed     AS ps_is_closed,
                ps.last_checked  AS ps_last_checked,
                b.*
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
             WHERE ps.program_id = $1",
        )
        .bind(pid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(|r| {
            let state = ProgramStateRow {
                program_id: r.get("ps_program_id"),
                on_chain_hash: r.get("ps_on_chain_hash"),
                authority: r.get("ps_authority"),
                is_frozen: r.get("ps_is_frozen"),
                is_closed: r.get("ps_is_closed"),
                last_checked: r.get("ps_last_checked"),
            };
            // LEFT JOIN LATERAL with LIMIT 1 → build columns are nullable.
            let build = r
                .try_get::<Uuid, _>("id")
                .ok()
                .map(|_| BuildRow::from_row(r));
            (state, build)
        }))
    }

    /// Cached on-chain state for a program.
    pub async fn get_program_state(&self, program_id: &str) -> Result<Option<ProgramStateRow>> {
        let row = sqlx::query("SELECT * FROM program_state WHERE program_id = $1")
            .bind(program_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(ProgramStateRow::from_row))
    }

    /// Full refresh from a snapshot. A `None` hash on the snapshot preserves
    /// the existing column rather than clobbering it, so a transient hash
    /// fetch failure doesn't lose previously known data.
    pub async fn upsert_program_state(
        &self,
        program_id: &str,
        state: &ProgramOnchainState,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO program_state
                (program_id, on_chain_hash, authority, is_frozen, is_closed, last_checked)
             VALUES ($1, $2, $3, $4, $5, NOW())
             ON CONFLICT (program_id) DO UPDATE
             SET on_chain_hash = COALESCE(EXCLUDED.on_chain_hash, program_state.on_chain_hash),
                 authority     = EXCLUDED.authority,
                 is_frozen     = EXCLUDED.is_frozen,
                 is_closed     = EXCLUDED.is_closed,
                 last_checked  = NOW()",
        )
        .bind(program_id)
        .bind(state.executable_hash.as_deref())
        .bind(&state.authority)
        .bind(state.is_frozen)
        .bind(state.is_closed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Records a program as closed and clears its authority.
    pub async fn mark_closed(&self, program_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO program_state (program_id, is_closed, last_checked)
             VALUES ($1, TRUE, NOW())
             ON CONFLICT (program_id) DO UPDATE
             SET is_closed = TRUE, authority = NULL, last_checked = NOW()",
        )
        .bind(program_id)
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

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT b.program_id)
             FROM builds b
             LEFT JOIN program_state ps ON ps.program_id = b.program_id
             WHERE b.status = 'completed'
               AND b.executable_hash IS NOT NULL
               AND b.executable_hash = ps.on_chain_hash
               AND NOT COALESCE(ps.is_closed, FALSE)
               AND NOT COALESCE(ps.is_frozen, FALSE)
               AND ($1 = '' OR b.program_id ILIKE $2 OR b.repository ILIKE $2)",
        )
        .bind(search)
        .bind(&pattern)
        .fetch_one(&self.pool)
        .await?;

        let ids: Vec<String> = sqlx::query_scalar(
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
        )
        .bind(search)
        .bind(&pattern)
        .bind(PER_PAGE)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok((ids, total))
    }

    /// Latest completed build paired with its state, for every program
    /// currently verified and not closed/frozen.
    pub async fn verified_programs_with_state(&self) -> Result<Vec<(BuildRow, ProgramStateRow)>> {
        let rows = sqlx::query(
            "SELECT b.*, ps.program_id as ps_program_id, ps.on_chain_hash as ps_on_chain_hash,
                    ps.authority as ps_authority, ps.is_frozen as ps_is_frozen,
                    ps.is_closed as ps_is_closed, ps.last_checked as ps_last_checked
             FROM (
                SELECT DISTINCT ON (program_id) *
                FROM builds
                WHERE status = 'completed' AND executable_hash IS NOT NULL
                ORDER BY program_id, completed_at DESC
             ) b
             JOIN program_state ps ON ps.program_id = b.program_id
             WHERE b.executable_hash = ps.on_chain_hash
               AND NOT ps.is_closed
               AND NOT ps.is_frozen",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                let build = BuildRow::from_row(row);
                let state = ProgramStateRow {
                    program_id: row.get("ps_program_id"),
                    on_chain_hash: row.get("ps_on_chain_hash"),
                    authority: row.get("ps_authority"),
                    is_frozen: row.get("ps_is_frozen"),
                    is_closed: row.get("ps_is_closed"),
                    last_checked: row.get("ps_last_checked"),
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
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT program_id FROM (
                SELECT ps.program_id, ps.last_checked
                FROM program_state ps
                UNION
                SELECT b.program_id, NULL::timestamptz AS last_checked
                FROM (SELECT DISTINCT program_id FROM builds WHERE status = 'completed') b
                WHERE NOT EXISTS (SELECT 1 FROM program_state ps WHERE ps.program_id = b.program_id)
             ) q
             ORDER BY last_checked ASC NULLS FIRST",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(ids)
    }

    /// Proxy for "is the sweep still running" — used by the health endpoints.
    pub async fn last_sweep_at(&self) -> Result<Option<DateTime<Utc>>> {
        let v: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT MAX(last_checked) FROM program_state")
                .fetch_one(&self.pool)
                .await?;
        Ok(v)
    }

    /// Records the on-disk log filename for a failed build.
    pub async fn insert_build_log(
        &self,
        build_id: Uuid,
        program_id: &str,
        file_name: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO build_logs (id, program_id, file_name) VALUES ($1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET file_name = EXCLUDED.file_name",
        )
        .bind(build_id)
        .bind(program_id)
        .bind(file_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Looks up the on-disk log filename for a build.
    pub async fn get_build_log_file(&self, build_id: Uuid) -> Result<Option<String>> {
        let file: Option<String> = sqlx::query_scalar(
            "SELECT file_name FROM build_logs
             WHERE id = $1
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(build_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(file)
    }
}
