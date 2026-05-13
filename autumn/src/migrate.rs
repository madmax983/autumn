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
        match run_pending(database_url, migrations) {
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
}
