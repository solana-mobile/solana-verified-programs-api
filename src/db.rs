//! Postgres data layer. Two tables of substance: `builds` (one row per
//! verification attempt — job + result merged) and `program_state` (one
//! cached row per program). Queries are typed via Diesel's DSL where it's
//! ergonomic; LATERAL joins fall back to `sql_query` (one query).

use crate::{
    error::{ApiError, Result},
    onchain::ProgramOnchainState,
    schema::{build_logs, builds, program_state},
    types::ProgramId,
};
use chrono::{DateTime, Utc};
use diesel::{
    dsl::{count_distinct, max, sql},
    prelude::*,
    sql_types::{Bool, Nullable, Text},
};
use diesel_async::{
    pooled_connection::{deadpool::Pool, AsyncDieselConnectionManager},
    AsyncPgConnection, RunQueryDsl,
};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use tracing::info;
use uuid::Uuid;

pub const PER_PAGE: i64 = 20;
pub const BUILD_STATUS_IN_PROGRESS: &str = "in_progress";
pub const BUILD_STATUS_COMPLETED: &str = "completed";
pub const BUILD_STATUS_FAILED: &str = "failed";

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");

#[derive(Clone)]
pub struct Db {
    pool: Pool<AsyncPgConnection>,
}

impl Db {
    /// Opens a bounded connection pool against `url`.
    pub async fn connect(url: &str, max_size: usize) -> Result<Self> {
        let mgr = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
        let pool = Pool::builder(mgr)
            .max_size(max_size)
            .build()
            .map_err(|e| ApiError::Internal(format!("pool build: {e}")))?;
        Ok(Self { pool })
    }

    /// Runs all pending embedded migrations.
    pub async fn migrate(&self, url: &str) -> Result<()> {
        // diesel-async doesn't have a migration harness yet (as of 0.5), so we
        // run them on a one-shot sync connection.
        let url = url.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut conn = diesel::PgConnection::establish(&url)
                .map_err(|e| ApiError::Internal(format!("migrate connect: {e}")))?;
            conn.run_pending_migrations(MIGRATIONS)
                .map_err(|e| ApiError::Internal(format!("migrate: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| ApiError::Internal(format!("migrate join: {e}")))??;
        info!("migrations applied");
        Ok(())
    }

    /// `SELECT 1` for the health endpoint.
    pub async fn ping(&self) -> Result<()> {
        let mut conn = self.conn().await?;
        diesel::sql_query("SELECT 1")
            .execute(&mut conn)
            .await
            .map_err(ApiError::from)?;
        Ok(())
    }

    async fn conn(
        &self,
    ) -> Result<diesel_async::pooled_connection::deadpool::Object<AsyncPgConnection>> {
        self.pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("pool acquire: {e}")))
    }
}

impl From<diesel::result::Error> for ApiError {
    fn from(e: diesel::result::Error) -> Self {
        match e {
            diesel::result::Error::NotFound => ApiError::NotFound("not found".into()),
            other => ApiError::Internal(format!("db: {other}")),
        }
    }
}

/// One row of `builds`. Selectable + Insertable so the same struct can drive
/// reads and writes when fields align.
#[derive(Debug, Clone, PartialEq, Eq, Queryable, Selectable, Identifiable)]
#[diesel(table_name = builds, primary_key(id))]
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

/// Subset of `program_state` callers actually read.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = program_state)]
pub struct ProgramStateRow {
    pub on_chain_hash: Option<String>,
    pub is_frozen: Option<bool>,
    pub is_closed: bool,
}

/// Insert-shape for `builds`. Two reasons it differs from [`BuildRow`]:
///
/// 1. Four columns are omitted because they're set later — `created_at` by
///    the DB's `DEFAULT NOW()`, then `executable_hash` / `completed_at` by
///    [`Db::mark_build_completed`] and `error_message` / `completed_at` by
///    [`Db::mark_build_failed`].
/// 2. Borrowed `&str` / `&[String]` instead of owned values — the caller
///    holds [`NewBuild`] and we want to avoid cloning into the insert.
#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = builds)]
struct NewBuildRow<'a> {
    id: Uuid,
    repository: &'a str,
    commit_hash: Option<&'a str>,
    program_id: &'a str,
    lib_name: Option<&'a str>,
    base_docker_image: Option<&'a str>,
    mount_path: Option<&'a str>,
    cargo_args: Option<&'a [String]>,
    bpf_flag: bool,
    arch: Option<&'a str>,
    signer: Option<&'a str>,
    status: &'a str,
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
        let row = NewBuildRow {
            id,
            repository: &b.repository,
            commit_hash: b.commit_hash.as_deref(),
            program_id: &b.program_id,
            lib_name: b.lib_name.as_deref(),
            base_docker_image: b.base_docker_image.as_deref(),
            mount_path: b.mount_path.as_deref(),
            cargo_args: b.cargo_args.as_deref(),
            bpf_flag: b.bpf_flag,
            arch: b.arch.as_deref(),
            signer: b.signer.as_deref(),
            status: BUILD_STATUS_IN_PROGRESS,
        };
        let mut conn = self.conn().await?;
        diesel::insert_into(builds::table)
            .values(&row)
            .execute(&mut conn)
            .await?;
        Ok(id)
    }

    /// Transitions a build to `completed` and records its executable hash.
    pub async fn mark_build_completed(&self, build_id: Uuid, hash: &str) -> Result<()> {
        let mut conn = self.conn().await?;
        diesel::update(builds::table.find(build_id))
            .set((
                builds::status.eq(BUILD_STATUS_COMPLETED),
                builds::executable_hash.eq(hash),
                builds::completed_at.eq(diesel::dsl::now),
            ))
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    /// Transitions a build to `failed` with the given error message.
    pub async fn mark_build_failed(&self, build_id: Uuid, error: &str) -> Result<()> {
        let mut conn = self.conn().await?;
        diesel::update(builds::table.find(build_id))
            .set((
                builds::status.eq(BUILD_STATUS_FAILED),
                builds::error_message.eq(error),
                builds::completed_at.eq(diesel::dsl::now),
            ))
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    /// Fetches a build by id.
    pub async fn get_build(&self, build_id: Uuid) -> Result<Option<BuildRow>> {
        let mut conn = self.conn().await?;
        builds::table
            .find(build_id)
            .select(BuildRow::as_select())
            .first(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    /// Most recent non-failed build with identical params. Failed rows are
    /// ignored — they're retryable.
    pub async fn find_duplicate(&self, b: &NewBuild) -> Result<Option<BuildRow>> {
        let mut conn = self.conn().await?;
        builds::table
            .filter(builds::program_id.eq(&b.program_id))
            .filter(builds::repository.eq(&b.repository))
            .filter(builds::commit_hash.is_not_distinct_from(&b.commit_hash))
            .filter(builds::lib_name.is_not_distinct_from(&b.lib_name))
            .filter(builds::base_docker_image.is_not_distinct_from(&b.base_docker_image))
            .filter(builds::mount_path.is_not_distinct_from(&b.mount_path))
            .filter(builds::cargo_args.is_not_distinct_from(&b.cargo_args))
            .filter(builds::bpf_flag.eq(b.bpf_flag))
            .filter(builds::arch.is_not_distinct_from(&b.arch))
            .filter(builds::signer.is_not_distinct_from(&b.signer))
            .filter(builds::status.ne(BUILD_STATUS_FAILED))
            .order(builds::created_at.desc())
            .select(BuildRow::as_select())
            .first(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    /// One row per signer — the signer's most recent completed claim for the program.
    pub async fn completed_builds_by_signer(
        &self,
        program_id: &ProgramId,
    ) -> Result<Vec<BuildRow>> {
        let mut conn = self.conn().await?;
        builds::table
            .filter(builds::program_id.eq(program_id.as_str()))
            .filter(builds::status.eq(BUILD_STATUS_COMPLETED))
            .distinct_on(builds::signer)
            .order((builds::signer, builds::completed_at.desc()))
            .select(BuildRow::as_select())
            .load(&mut conn)
            .await
            .map_err(Into::into)
    }

    /// Every completed build with this executable hash.
    pub async fn builds_by_executable_hash(&self, hash: &str) -> Result<Vec<BuildRow>> {
        let mut conn = self.conn().await?;
        builds::table
            .filter(builds::executable_hash.eq(hash))
            .filter(builds::status.eq(BUILD_STATUS_COMPLETED))
            .order(builds::completed_at.desc())
            .select(BuildRow::as_select())
            .load(&mut conn)
            .await
            .map_err(Into::into)
    }

    /// Most recent completed build for the program. When `prefer_hash` is
    /// set, prefers a build whose `executable_hash` matches; falls back to
    /// the latest of any hash.
    pub async fn best_build(
        &self,
        program_id: &ProgramId,
        prefer_hash: Option<&str>,
    ) -> Result<Option<BuildRow>> {
        let mut conn = self.conn().await?;
        // `IS NOT DISTINCT FROM` against a parameter needs `.bind()`-style SQL
        // because the DSL's `is_not_distinct_from` builds a Bool expression
        // but doesn't fit naturally inside ORDER BY (needs `.desc()`).
        let prefer_match = sql::<Bool>("(executable_hash IS NOT DISTINCT FROM ")
            .bind::<Nullable<Text>, _>(prefer_hash)
            .sql(")")
            .desc();
        builds::table
            .filter(builds::program_id.eq(program_id.as_str()))
            .filter(builds::status.eq(BUILD_STATUS_COMPLETED))
            .order((prefer_match, builds::completed_at.desc()))
            .select(BuildRow::as_select())
            .first(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    /// Cached on-chain state for a program.
    pub async fn get_program_state(&self, program_id: &str) -> Result<Option<ProgramStateRow>> {
        let mut conn = self.conn().await?;
        program_state::table
            .find(program_id)
            .select(ProgramStateRow::as_select())
            .first(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }

    /// Full refresh from a snapshot. A `None` hash on the snapshot preserves
    /// the existing column rather than clobbering it.
    pub async fn upsert_program_state(
        &self,
        program_id: &str,
        state: &ProgramOnchainState,
    ) -> Result<()> {
        let mut conn = self.conn().await?;
        diesel::sql_query(
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
        .bind::<Text, _>(program_id)
        .bind::<Nullable<Text>, _>(state.executable_hash.as_deref())
        .bind::<Nullable<Text>, _>(state.authority.as_deref())
        .bind::<Bool, _>(state.is_frozen)
        .bind::<Bool, _>(state.is_closed)
        .execute(&mut conn)
        .await?;
        Ok(())
    }

    /// Records a program as closed and clears its authority.
    pub async fn mark_closed(&self, program_id: &str) -> Result<()> {
        let mut conn = self.conn().await?;
        diesel::sql_query(
            "INSERT INTO program_state (program_id, is_closed, last_checked)
             VALUES ($1, TRUE, NOW())
             ON CONFLICT (program_id) DO UPDATE
             SET is_closed = TRUE, authority = NULL, last_checked = NOW()",
        )
        .bind::<Text, _>(program_id)
        .execute(&mut conn)
        .await?;
        Ok(())
    }

    /// One page of currently-verified program IDs plus the total count.
    pub async fn verified_programs_page(
        &self,
        page: i64,
        search: &str,
    ) -> Result<(Vec<String>, i64)> {
        let page = page.max(1);
        let offset = (page - 1) * PER_PAGE;
        let pattern = format!("%{search}%");
        let mut conn = self.conn().await?;

        // Diesel's boxed query for a JOIN isn't Clone, so we build it twice
        // via a closure. Cheap — it's just a builder tree, not a query.
        let build_q = || {
            let q = builds::table
                .inner_join(
                    program_state::table.on(program_state::program_id.eq(builds::program_id)),
                )
                .filter(builds::status.eq(BUILD_STATUS_COMPLETED))
                .filter(builds::executable_hash.is_not_null())
                .filter(builds::executable_hash.is_not_distinct_from(program_state::on_chain_hash))
                .filter(program_state::is_closed.eq(false))
                .filter(program_state::is_frozen.eq(false))
                .into_boxed();
            if search.is_empty() {
                q
            } else {
                q.filter(
                    builds::program_id
                        .ilike(pattern.clone())
                        .or(builds::repository.ilike(pattern.clone())),
                )
            }
        };

        let total = build_q()
            .select(count_distinct(builds::program_id))
            .first::<i64>(&mut conn)
            .await?;

        let ids = build_q()
            .select(builds::program_id)
            .distinct()
            .order(builds::program_id.asc())
            .limit(PER_PAGE)
            .offset(offset)
            .load::<String>(&mut conn)
            .await?;

        Ok((ids, total))
    }

    /// Latest completed build for every program that's currently verified
    /// (build hash matches on-chain hash) and not closed/frozen.
    pub async fn currently_verified_builds(&self) -> Result<Vec<BuildRow>> {
        let mut conn = self.conn().await?;
        builds::table
            .inner_join(program_state::table.on(program_state::program_id.eq(builds::program_id)))
            .filter(builds::status.eq(BUILD_STATUS_COMPLETED))
            .filter(builds::executable_hash.is_not_null())
            .filter(builds::executable_hash.is_not_distinct_from(program_state::on_chain_hash))
            .filter(program_state::is_closed.eq(false))
            .filter(program_state::is_frozen.eq(false))
            .distinct_on(builds::program_id)
            .order((builds::program_id, builds::completed_at.desc()))
            .select(BuildRow::as_select())
            .load(&mut conn)
            .await
            .map_err(Into::into)
    }

    /// Every program ID the sweep should refresh.
    pub async fn sweep_program_ids(&self) -> Result<Vec<String>> {
        // A UNION + NOT EXISTS subquery is awkward in the DSL; raw SQL is the
        // pragmatic shape.
        #[derive(diesel::QueryableByName)]
        struct Row {
            #[diesel(sql_type = Text)]
            program_id: String,
        }
        let mut conn = self.conn().await?;
        let rows: Vec<Row> = diesel::sql_query(
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
        .load(&mut conn)
        .await?;
        Ok(rows.into_iter().map(|r| r.program_id).collect())
    }

    /// Proxy for "is the sweep still running" — used by the health endpoints.
    pub async fn last_sweep_at(&self) -> Result<Option<DateTime<Utc>>> {
        let mut conn = self.conn().await?;
        program_state::table
            .select(max(program_state::last_checked))
            .first(&mut conn)
            .await
            .map_err(Into::into)
    }

    /// Records the on-disk log filename for a failed build.
    pub async fn insert_build_log(
        &self,
        build_id: Uuid,
        program_id: &str,
        file_name: &str,
    ) -> Result<()> {
        let mut conn = self.conn().await?;
        diesel::insert_into(build_logs::table)
            .values((
                build_logs::id.eq(build_id),
                build_logs::program_id.eq(program_id),
                build_logs::file_name.eq(file_name),
            ))
            .on_conflict(build_logs::id)
            .do_update()
            .set(build_logs::file_name.eq(file_name))
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    /// Looks up the on-disk log filename for a build.
    pub async fn get_build_log_file(&self, build_id: Uuid) -> Result<Option<String>> {
        let mut conn = self.conn().await?;
        build_logs::table
            .find(build_id)
            .order(build_logs::created_at.desc())
            .select(build_logs::file_name)
            .first(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }
}
