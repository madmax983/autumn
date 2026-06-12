//! Database migration support.
//!
//! Provides helpers for running Diesel migrations at application startup.
//! In **dev** mode, pending migrations run automatically; in **prod** mode,
//! they must be applied explicitly via `autumn migrate`.
//!
//! # Usage
//!
//! Application code typically does not use this module directly. Instead,
//! pass embedded migrations to [`AppBuilder::migrations`](crate::app::AppBuilder::migrations)
//! and the framework handles the rest:
//!
//! ```rust,ignore
//! use diesel_migrations::{EmbeddedMigrations, embed_migrations};
//!
//! const MIGRATIONS: EmbeddedMigrations = embed_migrations!();
//!
//! #[autumn_web::main]
//! async fn main() {
//!     autumn_web::app()
//!         .routes(routes![...])
//!         .migrations(MIGRATIONS)
//!         .run()
//!         .await;
//! }
//! ```

use diesel::migration::Migration;
use diesel::{Connection, RunQueryDsl};
use diesel_migrations::{HarnessWithOutput, MigrationHarness};

/// Re-export `EmbeddedMigrations` so users can reference it without adding
/// `diesel_migrations` as a direct dependency.
pub use diesel_migrations::EmbeddedMigrations;

/// Re-export the `embed_migrations!` macro.
pub use diesel_migrations::embed_migrations;

/// Embedded Autumn framework migrations.
///
/// These are applied by `autumn migrate` and are also registered
/// automatically at startup when a framework feature requires its own table.
pub const FRAMEWORK_MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Result of running pending migrations.
#[derive(Debug)]
pub struct MigrationResult {
    /// Names of the migrations that were applied.
    pub applied: Vec<String>,
}

/// Error type for migration operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrationError {
    /// Failed to connect to the database.
    #[error("failed to connect to database: {0}")]
    Connection(String),

    /// A migration failed to apply.
    #[error("migration failed: {0}")]
    Migration(String),

    /// The migration advisory lock could not be acquired within the timeout.
    ///
    /// Another process is likely running migrations. Increase `wait_timeout`
    /// or investigate the blocking session in `pg_locks`.
    #[error(
        "migration advisory lock not acquired within {timeout_secs}s; \
         another process may still be running migrations"
    )]
    LockTimeout {
        /// Configured wait timeout in seconds.
        timeout_secs: u64,
    },
}

/// `PostgreSQL` advisory lock key used to serialize concurrent migration runs.
///
/// Derived from the big-endian encoding of the ASCII bytes `autn_mig` (`i64`).
/// The value is stable across framework versions so operators can monitor
/// contention without consulting source code.
///
/// Monitor contention with:
///
/// ```sql
/// SELECT pid, granted, mode
/// FROM pg_locks
/// WHERE locktype = 'advisory'
///   AND classid = 1635087470
///   AND objid   = 1601005927
///   AND objsubid = 1;
/// ```
pub const MIGRATION_ADVISORY_LOCK_KEY: i64 = 0x6175_746E_5F6D_6967_u64.cast_signed();

/// Default time to wait for the migration advisory lock before failing.
///
/// Override per call via the `wait_timeout` parameter of [`run_pending_locked`]
/// or [`hold_migration_lock`].
pub const DEFAULT_LOCK_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(diesel::QueryableByName)]
struct AdvisoryLockRow {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    acquired: bool,
}

#[derive(diesel::QueryableByName)]
struct AdvisoryUnlockRow {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    released: bool,
}

#[derive(diesel::QueryableByName)]
struct AppliedMigrationVersion {
    #[diesel(sql_type = diesel::sql_types::Text)]
    version: String,
}

/// Runtime readiness state for a configured read replica's schema version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplicaMigrationReadiness {
    /// Primary and replica report the same applied migration versions.
    Ready,
    /// The replica is reachable but has not applied the same migrations.
    Stale {
        primary_latest: Option<String>,
        replica_latest: Option<String>,
    },
    /// The framework could not determine replica migration state.
    Unknown(String),
}

impl ReplicaMigrationReadiness {
    /// Returns whether the replica can safely receive read traffic.
    #[must_use]
    pub(crate) const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Human-readable reason used in runtime readiness state.
    #[must_use]
    pub(crate) fn detail(&self) -> Option<String> {
        match self {
            Self::Ready => None,
            Self::Stale {
                primary_latest,
                replica_latest,
            } => Some(format!(
                "replica migrations lag primary (primary_latest={}, replica_latest={})",
                primary_latest.as_deref().unwrap_or("<none>"),
                replica_latest.as_deref().unwrap_or("<none>")
            )),
            Self::Unknown(error) => Some(format!("replica migration readiness unknown: {error}")),
        }
    }
}

/// Run all pending migrations against the given database URL.
///
/// Uses a **synchronous** `PgConnection` (not the async pool) because
/// Diesel migrations require `MigrationHarness`, which is sync-only.
///
/// Returns the list of migration versions that were applied, or an error
/// if a migration fails (including the failing SQL in the message).
///
/// # Errors
///
/// Returns [`MigrationError::Connection`] if the database is unreachable,
/// or [`MigrationError::Migration`] if a migration fails.
pub fn run_pending(
    database_url: &str,
    migrations: EmbeddedMigrations,
) -> Result<MigrationResult, MigrationError> {
    let mut conn = diesel::PgConnection::establish(database_url)
        .map_err(|e| MigrationError::Connection(e.to_string()))?;

    let mut harness = HarnessWithOutput::write_to_stdout(&mut conn);

    let applied = harness
        .run_pending_migrations(migrations)
        .map_err(|e| MigrationError::Migration(e.to_string()))?;

    Ok(MigrationResult {
        applied: applied.iter().map(|m| format!("{m}")).collect(),
    })
}

/// Return names of pending (not yet applied) migrations.
///
/// # Errors
///
/// Returns [`MigrationError::Connection`] if the database is unreachable,
/// or [`MigrationError::Migration`] if status cannot be determined.
pub fn pending_migrations(
    database_url: &str,
    migrations: EmbeddedMigrations,
) -> Result<Vec<String>, MigrationError> {
    let mut conn = diesel::PgConnection::establish(database_url)
        .map_err(|e| MigrationError::Connection(e.to_string()))?;

    let pending = conn
        .pending_migrations(migrations)
        .map_err(|e| MigrationError::Migration(e.to_string()))?;

    Ok(pending
        .iter()
        .map(|m| m.name().version().to_string())
        .collect())
}

pub(crate) fn compare_replica_migration_versions(
    primary: &[String],
    replica: &[String],
) -> ReplicaMigrationReadiness {
    let primary_versions: std::collections::BTreeSet<_> = primary.iter().collect();
    let replica_versions: std::collections::BTreeSet<_> = replica.iter().collect();

    if primary_versions == replica_versions {
        ReplicaMigrationReadiness::Ready
    } else {
        ReplicaMigrationReadiness::Stale {
            primary_latest: primary_versions
                .iter()
                .next_back()
                .map(|version| (*version).clone()),
            replica_latest: replica_versions
                .iter()
                .next_back()
                .map(|version| (*version).clone()),
        }
    }
}

fn applied_migration_versions(database_url: &str) -> Result<Vec<String>, MigrationError> {
    let mut conn = diesel::PgConnection::establish(database_url)
        .map_err(|e| MigrationError::Connection(e.to_string()))?;

    let rows = diesel::sql_query("SELECT version FROM __diesel_schema_migrations ORDER BY version")
        .load::<AppliedMigrationVersion>(&mut conn)
        .map_err(|e| MigrationError::Migration(e.to_string()))?;

    Ok(rows.into_iter().map(|row| row.version).collect())
}

pub(crate) fn check_replica_migration_readiness(
    primary_url: &str,
    replica_url: &str,
) -> ReplicaMigrationReadiness {
    let primary = match applied_migration_versions(primary_url) {
        Ok(versions) => versions,
        Err(error) => return ReplicaMigrationReadiness::Unknown(error.to_string()),
    };
    let replica = match applied_migration_versions(replica_url) {
        Ok(versions) => versions,
        Err(error) => return ReplicaMigrationReadiness::Unknown(error.to_string()),
    };

    compare_replica_migration_versions(&primary, &replica)
}

pub(crate) async fn check_replica_migration_readiness_blocking(
    primary_url: String,
    replica_url: String,
) -> ReplicaMigrationReadiness {
    tokio::task::spawn_blocking(move || {
        check_replica_migration_readiness(&primary_url, &replica_url)
    })
    .await
    .unwrap_or_else(|error| {
        ReplicaMigrationReadiness::Unknown(format!(
            "replica migration readiness task failed: {error}"
        ))
    })
}

/// Acquire the `PostgreSQL` session-level advisory lock that serializes migration runs.
///
/// Polls `pg_try_advisory_lock` at 500 ms intervals until the lock is
/// acquired or `timeout` elapses. Logs at `INFO` on acquisition and `DEBUG`
/// while waiting.
///
/// **Non-`PostgreSQL` note:** advisory locks are a `PostgreSQL`-specific primitive.
/// `SQLite` and in-memory test harnesses do not support them. Those backends are
/// single-process by nature; `run_pending` (the unlocked variant) is the right
/// choice there.
///
/// # Errors
///
/// Returns [`MigrationError::Migration`] if the database query fails, or
/// [`MigrationError::LockTimeout`] if the lock is not acquired within `timeout`.
pub fn acquire_migration_lock(
    conn: &mut diesel::PgConnection,
    timeout: std::time::Duration,
) -> Result<(), MigrationError> {
    let start = std::time::Instant::now();
    let poll = std::time::Duration::from_millis(500);

    tracing::info!(
        lock_key = MIGRATION_ADVISORY_LOCK_KEY,
        timeout_secs = timeout.as_secs(),
        "Acquiring migration advisory lock",
    );

    loop {
        let acquired = diesel::sql_query("SELECT pg_try_advisory_lock($1) AS acquired")
            .bind::<diesel::sql_types::BigInt, _>(MIGRATION_ADVISORY_LOCK_KEY)
            .get_result::<AdvisoryLockRow>(conn)
            .map_err(|e| MigrationError::Migration(e.to_string()))?
            .acquired;

        if acquired {
            tracing::info!("Migration advisory lock acquired");
            return Ok(());
        }

        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return Err(MigrationError::LockTimeout {
                timeout_secs: timeout.as_secs(),
            });
        }

        tracing::debug!(
            elapsed_secs = elapsed.as_secs(),
            timeout_secs = timeout.as_secs(),
            "Waiting for migration advisory lock; another process may be running migrations",
        );

        std::thread::sleep(poll.min(timeout.saturating_sub(elapsed)));
    }
}

/// Release the `PostgreSQL` session-level advisory lock acquired by
/// [`acquire_migration_lock`].
///
/// Called automatically by [`MigrationLockGuard`] on drop. Logs at `INFO` on
/// success and `WARN` if the lock was not held or the query fails. `PostgreSQL`
/// also releases session-level advisory locks automatically when the connection
/// closes, so a missed explicit release is safe.
pub fn release_migration_lock(conn: &mut diesel::PgConnection) {
    match diesel::sql_query("SELECT pg_advisory_unlock($1) AS released")
        .bind::<diesel::sql_types::BigInt, _>(MIGRATION_ADVISORY_LOCK_KEY)
        .get_result::<AdvisoryUnlockRow>(conn)
    {
        Ok(row) if row.released => {
            tracing::info!("Migration advisory lock released");
        }
        Ok(_) => {
            tracing::warn!("Migration advisory unlock returned false: lock was not held");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to release migration advisory lock");
        }
    }
}

/// RAII guard that holds a `PostgreSQL` advisory lock for the duration of a
/// migration run.
///
/// Created by [`hold_migration_lock`]. The lock is released when this guard
/// drops, or automatically when the underlying connection closes on process
/// exit (so `std::process::exit` is safe).
///
/// # Non-`PostgreSQL` backends
///
/// `SQLite` and in-memory test harnesses do not support advisory locks and do
/// not need cross-process serialization (they are single-process by nature).
/// Skip this guard when running against those backends.
pub struct MigrationLockGuard {
    conn: diesel::PgConnection,
}

impl std::fmt::Debug for MigrationLockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationLockGuard").finish_non_exhaustive()
    }
}

impl Drop for MigrationLockGuard {
    fn drop(&mut self) {
        release_migration_lock(&mut self.conn);
    }
}

/// Open a new Postgres connection and acquire the migration advisory lock,
/// returning a [`MigrationLockGuard`] that releases it on drop.
///
/// This is the right primitive when migrations are run by an external process
/// (e.g. the `diesel` CLI subprocess in `autumn migrate run`): the guard keeps
/// the lock connection alive for the duration of the external run.
///
/// Use [`run_pending_locked`] when the Rust harness runs migrations directly.
///
/// # Errors
///
/// Returns [`MigrationError::Connection`] if the database is unreachable, or
/// [`MigrationError::LockTimeout`] if the lock cannot be acquired within
/// `wait_timeout`.
pub fn hold_migration_lock(
    database_url: &str,
    wait_timeout: std::time::Duration,
) -> Result<MigrationLockGuard, MigrationError> {
    let mut conn = diesel::PgConnection::establish(database_url)
        .map_err(|e| MigrationError::Connection(e.to_string()))?;

    acquire_migration_lock(&mut conn, wait_timeout)?;

    Ok(MigrationLockGuard { conn })
}

/// Run all pending migrations under a Postgres advisory lock.
///
/// Serializes concurrent migration attempts across processes: exactly one
/// process applies pending migrations while the rest wait, find no pending
/// work, and return a [`MigrationResult`] with an empty `applied` list.
///
/// The lock is acquired **before** the pending-migration list is read,
/// closing the check-then-apply race. It is released after the harness
/// commits or rolls back all migrations.
///
/// Pass `wait_timeout = None` to use [`DEFAULT_LOCK_WAIT_TIMEOUT`] (60 s).
///
/// # Non-`PostgreSQL` note
///
/// Advisory locks are `PostgreSQL`-specific. For `SQLite` or in-memory test
/// harnesses call [`run_pending`] directly — those backends are single-process
/// and do not require cross-process serialization.
///
/// # Errors
///
/// Returns [`MigrationError::Connection`] if the database is unreachable,
/// [`MigrationError::LockTimeout`] if the advisory lock cannot be acquired
/// within `wait_timeout`, or [`MigrationError::Migration`] if a migration
/// fails to apply.
pub fn run_pending_locked(
    database_url: &str,
    migrations: EmbeddedMigrations,
    wait_timeout: Option<std::time::Duration>,
) -> Result<MigrationResult, MigrationError> {
    let timeout = wait_timeout.unwrap_or(DEFAULT_LOCK_WAIT_TIMEOUT);

    let mut conn = diesel::PgConnection::establish(database_url)
        .map_err(|e| MigrationError::Connection(e.to_string()))?;

    acquire_migration_lock(&mut conn, timeout)?;

    // Collect migration names eagerly so the harness borrow on `conn` is
    // dropped before we call release_migration_lock on the same connection.
    let migration_result: Result<Vec<String>, MigrationError> = {
        let mut harness = HarnessWithOutput::write_to_stdout(&mut conn);
        harness
            .run_pending_migrations(migrations)
            .map(|applied| applied.iter().map(|m| format!("{m}")).collect())
            .map_err(|e| MigrationError::Migration(e.to_string()))
    };

    release_migration_lock(&mut conn);

    Ok(MigrationResult {
        applied: migration_result?,
    })
}

fn should_auto_apply(profile: Option<&str>, allow_auto_migrate_in_production: bool) -> bool {
    let profile_name = profile.unwrap_or("none");
    matches!(profile_name, "dev" | "development")
        || (matches!(profile_name, "prod" | "production") && allow_auto_migrate_in_production)
}

/// Run migrations according to the active profile and migration policy.
///
/// - **dev/development**: runs all pending migrations automatically and logs each one.
/// - **prod/production**: logs pending migrations unless
///   `allow_auto_migrate_in_production` is enabled.
/// - **other profiles**: logs pending migrations without auto-applying.
///
/// Called internally by [`AppBuilder::run`](crate::app::AppBuilder::run)
/// when migrations are registered via `.migrations()`.
#[allow(clippy::cognitive_complexity)]
pub(crate) fn auto_migrate(
    database_url: &str,
    profile: Option<&str>,
    allow_auto_migrate_in_production: bool,
    migrations: EmbeddedMigrations,
) {
    let profile_name = profile.unwrap_or("none");
    let is_dev = matches!(profile_name, "dev" | "development");
    let is_prod = matches!(profile_name, "prod" | "production");
    let should_auto_apply = should_auto_apply(profile, allow_auto_migrate_in_production);

    if should_auto_apply {
        if is_dev {
            tracing::info!("Development profile: running pending database migrations...");
        } else {
            tracing::warn!(
                profile = profile_name,
                "Production auto-migration is enabled; running pending database migrations"
            );
        }
        match run_pending_locked(database_url, migrations, None) {
            Ok(result) if result.applied.is_empty() => {
                tracing::info!("No pending migrations");
            }
            Ok(result) => {
                for name in &result.applied {
                    tracing::info!(migration = %name, "Applied migration");
                }
                tracing::info!(
                    count = result.applied.len(),
                    "All pending migrations applied"
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to run migrations");
                std::process::exit(1);
            }
        }
    } else {
        // In non-dev modes, just report status
        match pending_migrations(database_url, migrations) {
            Ok(pending) if pending.is_empty() => {
                tracing::info!("Database migrations are up to date");
            }
            Ok(pending) => {
                if is_prod {
                    tracing::warn!(
                        "Production profile detected: automatic migrations are disabled by default. \
                         Run `autumn migrate check` to review safety before applying, then \
                         `autumn migrate` in your deployment job. \
                         Set database.auto_migrate_in_production=true only for single-process \
                         deployments after confirming all pending migrations are safe for a \
                         rolling deploy (expand/contract pattern)."
                    );
                }
                tracing::warn!(
                    count = pending.len(),
                    "Pending migrations detected. Run `autumn migrate` to apply them."
                );
                for name in &pending {
                    tracing::warn!(migration = %name, "Pending migration");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Could not check migration status");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Red-phase tests for advisory-lock API (fail until implemented) ─────

    #[test]
    fn lock_timeout_error_display() {
        let err = MigrationError::LockTimeout { timeout_secs: 60 };
        let msg = err.to_string();
        assert!(msg.contains("60"), "message must contain the timeout value");
        assert!(
            msg.to_lowercase().contains("lock") || msg.to_lowercase().contains("timeout"),
            "message must mention lock or timeout: {msg}"
        );
    }

    #[test]
    fn migration_advisory_lock_key_is_positive_and_stable() {
        const { assert!(MIGRATION_ADVISORY_LOCK_KEY > 0) };
        // Exact value is part of the public API; it must not drift across versions.
        assert_eq!(
            MIGRATION_ADVISORY_LOCK_KEY,
            0x6175_746E_5F6D_6967_u64.cast_signed()
        );
    }

    #[test]
    fn default_lock_wait_timeout_is_sixty_seconds() {
        assert_eq!(DEFAULT_LOCK_WAIT_TIMEOUT.as_secs(), 60);
    }

    #[test]
    fn run_pending_locked_fails_with_connection_error_on_bad_url() {
        const MIGRATIONS: EmbeddedMigrations =
            diesel_migrations::embed_migrations!("../examples/todo-app/migrations");
        let url = "postgres://invalid_user:invalid_password@0.0.0.0:1/invalid_db";
        let result = run_pending_locked(url, MIGRATIONS, None);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), MigrationError::Connection(_)),
            "unreachable host must produce Connection error, not LockTimeout"
        );
    }

    /// Spawns 4 concurrent migration runners against a real Postgres container
    /// and asserts that exactly one applies the pending migrations while the
    /// rest find no pending work and exit successfully.
    #[cfg(feature = "test-support")]
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers)"]
    async fn four_concurrent_runners_serialize_and_exactly_one_applies() {
        use testcontainers::runners::AsyncRunner as _;
        use testcontainers_modules::postgres::Postgres;

        const TEST_MIGRATIONS: EmbeddedMigrations =
            diesel_migrations::embed_migrations!("../examples/todo-app/migrations");

        let container = Postgres::default()
            .start()
            .await
            .expect("failed to start Postgres testcontainer (is Docker running?)");

        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let url = url.clone();
                tokio::task::spawn_blocking(move || run_pending_locked(&url, TEST_MIGRATIONS, None))
            })
            .collect();

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.expect("task panicked"));
        }

        // (c) No runner should produce an error.
        for result in &results {
            assert!(
                result.is_ok(),
                "runner produced unexpected error: {result:?}"
            );
        }

        // (a) Exactly one runner applied migrations.
        let applied_count = results
            .iter()
            .filter(|r| r.as_ref().is_ok_and(|m| !m.applied.is_empty()))
            .count();
        assert_eq!(
            applied_count, 1,
            "exactly one runner should apply migrations; results={results:?}"
        );

        // (b) The final schema must include all expected tables.
        // We verify by checking that a subsequent run finds no pending migrations.
        let final_check =
            run_pending_locked(&url, TEST_MIGRATIONS, None).expect("post-run check failed");
        assert!(
            final_check.applied.is_empty(),
            "schema must be fully applied after concurrent run"
        );
    }

    // ── Existing tests ─────────────────────────────────────────────────────

    #[test]
    fn migration_result_debug() {
        let result = MigrationResult {
            applied: vec!["00000000000001".to_string()],
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("00000000000001"));
    }

    #[test]
    fn migration_error_display_connection() {
        let err = MigrationError::Connection("refused".to_string());
        let msg = err.to_string();
        assert!(msg.contains("connect"));
        assert!(msg.contains("refused"));
    }

    #[test]
    fn migration_error_display_migration() {
        let err = MigrationError::Migration("syntax error".to_string());
        let msg = err.to_string();
        assert!(msg.contains("migration failed"));
        assert!(msg.contains("syntax error"));
    }

    #[test]
    fn replica_migration_comparison_detects_stale_replica() {
        let primary = vec!["00000000000001".to_owned(), "00000000000002".to_owned()];
        let replica = vec!["00000000000001".to_owned()];

        let readiness = compare_replica_migration_versions(&primary, &replica);

        assert!(!readiness.is_ready());
        assert!(
            readiness
                .detail()
                .expect("stale detail")
                .contains("00000000000002")
        );
    }

    #[test]
    fn profile_aliases_are_recognized() {
        assert!(should_auto_apply(Some("dev"), false));
        assert!(should_auto_apply(Some("development"), false));
        assert!(!should_auto_apply(Some("prod"), false));
        assert!(!should_auto_apply(Some("production"), false));
        assert!(should_auto_apply(Some("prod"), true));
        assert!(should_auto_apply(Some("production"), true));
        assert!(!should_auto_apply(Some("staging"), true));
    }

    #[test]
    fn run_pending_connection_error() {
        const MIGRATIONS: EmbeddedMigrations =
            diesel_migrations::embed_migrations!("../examples/todo-app/migrations");
        let url = "postgres://invalid_user:invalid_password@0.0.0.0:1/invalid_db";
        let result = run_pending(url, MIGRATIONS);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), MigrationError::Connection(_)));
    }

    #[test]
    fn replica_migration_readiness_ready_is_ready_and_has_no_detail() {
        assert!(ReplicaMigrationReadiness::Ready.is_ready());
        assert_eq!(ReplicaMigrationReadiness::Ready.detail(), None);
    }

    #[test]
    fn replica_migration_readiness_unknown_is_not_ready_and_has_detail() {
        let r = ReplicaMigrationReadiness::Unknown("db error xyz".to_string());
        assert!(!r.is_ready());
        let detail = r.detail().expect("Unknown must have detail");
        assert!(
            detail.contains("db error xyz"),
            "detail must contain the error: {detail}"
        );
    }

    #[test]
    fn compare_migration_versions_equal_returns_ready() {
        let versions = vec!["00000000000001".to_owned(), "00000000000002".to_owned()];
        let readiness = compare_replica_migration_versions(&versions, &versions.clone());
        assert!(readiness.is_ready());
        assert_eq!(readiness.detail(), None);
    }

    #[test]
    fn hold_migration_lock_fails_with_connection_error_on_bad_url() {
        let result = hold_migration_lock(
            "postgres://invalid_user:invalid_password@0.0.0.0:1/invalid_db",
            DEFAULT_LOCK_WAIT_TIMEOUT,
        );
        assert!(
            matches!(result.unwrap_err(), MigrationError::Connection(_)),
            "unreachable host must produce Connection error"
        );
    }
}
