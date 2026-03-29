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

use diesel::Connection;
use diesel::migration::Migration;
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
pub enum MigrationError {
    /// Failed to connect to the database.
    #[error("failed to connect to database: {0}")]
    Connection(String),

    /// A migration failed to apply.
    #[error("migration failed: {0}")]
    Migration(String),
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

/// Run migrations according to the active profile.
///
/// - **dev**: runs all pending migrations automatically and logs each one.
/// - **prod** / other: logs a warning if pending migrations exist but does
///   not apply them.
///
/// Called internally by [`AppBuilder::run`](crate::app::AppBuilder::run)
/// when migrations are registered via `.migrations()`.
pub(crate) fn auto_migrate(
    database_url: &str,
    profile: Option<&str>,
    migrations: EmbeddedMigrations,
) {
    let is_dev = profile == Some("dev");

    if is_dev {
        tracing::info!("Dev mode: running pending database migrations...");
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
}
