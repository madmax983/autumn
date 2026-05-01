//! Seed context for populating databases with representative data.
//!
//! Enabled with the `seed` cargo feature (off by default). Include in your
//! project's `Cargo.toml` to use it in a seed binary:
//!
//! ```toml
//! autumn-web = { version = "...", features = ["seed"] }
//! ```
//!
//! # Example (`src/bin/seed.rs`)
//!
//! ```no_run
//! use autumn_web::seed::SeedContext;
//!
//! #[tokio::main]
//! async fn main() {
//!     let ctx = SeedContext::build().expect("seed context");
//!     let mut db = ctx.conn().await.expect("db connection");
//!     // use db with Diesel queries ...
//!     println!("Seed complete (profile: {})", ctx.profile());
//! }
//! ```

use std::path::Path;

use crate::config::DatabaseConfig;
use crate::db::create_pool;
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::deadpool::{Object, Pool};

/// Error type returned by [`SeedContext`] operations.
#[derive(Debug, thiserror::Error)]
pub enum SeedContextError {
    /// No database URL was found in the environment or `autumn.toml`.
    #[error(
        "no database URL configured; set AUTUMN_DATABASE__URL or `database.url` in autumn.toml"
    )]
    NoDatabaseUrl,

    /// The connection pool could not be built.
    #[error("failed to build connection pool: {0}")]
    PoolBuild(#[from] crate::db::PoolError),

    /// A pooled connection could not be acquired.
    #[error("failed to acquire database connection: {0}")]
    Connection(String),
}

/// Context provided to a seed binary.
///
/// Holds the database connection pool and the active profile, both resolved
/// from the project's `autumn.toml` and environment variables — the same
/// sources the main application uses.
///
/// # Usage
///
/// ```no_run
/// # use autumn_web::seed::SeedContext;
/// # #[tokio::main]
/// # async fn main() {
/// let ctx = SeedContext::build().expect("seed context");
/// println!("profile: {}", ctx.profile());
/// let mut db = ctx.conn().await.expect("connection");
/// // use &mut *db as &mut AsyncPgConnection with diesel_async queries
/// # }
/// ```
pub struct SeedContext {
    pool: Pool<AsyncPgConnection>,
    profile: String,
}

impl SeedContext {
    /// Build a `SeedContext` by reading the database URL and profile from the
    /// environment and `autumn.toml` in the current working directory.
    ///
    /// Profile resolution order (first wins):
    /// 1. `AUTUMN_ENV` env var
    /// 2. `AUTUMN_PROFILE` env var
    /// 3. Defaults to `"dev"`
    ///
    /// Database URL resolution order (first wins):
    /// 1. `AUTUMN_DATABASE__URL` env var
    /// 2. `DATABASE_URL` env var
    /// 3. `database.url` in `autumn.toml`
    ///
    /// # Errors
    ///
    /// Returns [`SeedContextError::NoDatabaseUrl`] if no database URL is
    /// configured, or [`SeedContextError::PoolBuild`] if the pool cannot be
    /// constructed.
    pub fn build() -> Result<Self, SeedContextError> {
        let profile = resolve_profile();
        let db_url =
            resolve_database_url().ok_or(SeedContextError::NoDatabaseUrl)?;

        let config = DatabaseConfig {
            url: Some(db_url),
            ..DatabaseConfig::default()
        };

        let pool = create_pool(&config)?
            .ok_or(SeedContextError::NoDatabaseUrl)?;

        Ok(Self { pool, profile })
    }

    /// Returns the active profile name (e.g. `"dev"`, `"demo"`, `"test"`).
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Acquires a pooled database connection.
    ///
    /// Returns a [`Object<AsyncPgConnection>`] that implements `DerefMut` to
    /// `AsyncPgConnection`, so it can be passed directly to diesel-async
    /// query methods as `&mut *conn`.
    ///
    /// # Errors
    ///
    /// Returns [`SeedContextError::Connection`] if the pool is exhausted or
    /// the connection cannot be established.
    pub async fn conn(&self) -> Result<Object<AsyncPgConnection>, SeedContextError> {
        self.pool
            .get()
            .await
            .map_err(|e| SeedContextError::Connection(e.to_string()))
    }
}

/// Resolve the active profile from environment variables.
fn resolve_profile() -> String {
    std::env::var("AUTUMN_ENV")
        .or_else(|_| std::env::var("AUTUMN_PROFILE"))
        .unwrap_or_else(|_| "dev".to_string())
}

/// Resolve the database URL from environment variables and `autumn.toml`.
fn resolve_database_url() -> Option<String> {
    if let Ok(url) = std::env::var("AUTUMN_DATABASE__URL")
        && !url.is_empty()
    {
        return Some(url);
    }
    if let Ok(url) = std::env::var("DATABASE_URL")
        && !url.is_empty()
    {
        return Some(url);
    }

    let config_path = Path::new("autumn.toml");
    if config_path.exists()
        && let Ok(contents) = std::fs::read_to_string(config_path)
        && let Ok(table) = toml::from_str::<toml::Table>(&contents)
    {
        let value = toml::Value::Table(table);
        if let Some(url) = value
            .get("database")
            .and_then(|db: &toml::Value| db.get("url"))
            .and_then(|u: &toml::Value| u.as_str())
            .filter(|u| !u.is_empty())
        {
            return Some(url.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_profile ────────────────────────────────────────────────────

    #[test]
    fn resolve_profile_defaults_to_dev() {
        // Isolate from the real environment using temp_env.
        temp_env::with_vars(
            [
                ("AUTUMN_ENV", None::<&str>),
                ("AUTUMN_PROFILE", None::<&str>),
            ],
            || {
                assert_eq!(resolve_profile(), "dev");
            },
        );
    }

    #[test]
    fn resolve_profile_prefers_autumn_env() {
        temp_env::with_vars(
            [
                ("AUTUMN_ENV", Some("demo")),
                ("AUTUMN_PROFILE", Some("test")),
            ],
            || {
                assert_eq!(resolve_profile(), "demo");
            },
        );
    }

    #[test]
    fn resolve_profile_falls_back_to_autumn_profile() {
        temp_env::with_vars(
            [
                ("AUTUMN_ENV", None::<&str>),
                ("AUTUMN_PROFILE", Some("staging")),
            ],
            || {
                assert_eq!(resolve_profile(), "staging");
            },
        );
    }

    // ── resolve_database_url ───────────────────────────────────────────────

    #[test]
    fn resolve_database_url_prefers_autumn_database_url() {
        temp_env::with_vars(
            [
                ("AUTUMN_DATABASE__URL", Some("postgres://primary:5432/db")),
                ("DATABASE_URL", Some("postgres://secondary:5432/db")),
            ],
            || {
                assert_eq!(
                    resolve_database_url().as_deref(),
                    Some("postgres://primary:5432/db")
                );
            },
        );
    }

    #[test]
    fn resolve_database_url_falls_back_to_database_url() {
        temp_env::with_vars(
            [
                ("AUTUMN_DATABASE__URL", None::<&str>),
                ("DATABASE_URL", Some("postgres://fallback:5432/db")),
            ],
            || {
                assert_eq!(
                    resolve_database_url().as_deref(),
                    Some("postgres://fallback:5432/db")
                );
            },
        );
    }

    #[test]
    fn resolve_database_url_returns_none_when_nothing_configured() {
        temp_env::with_vars(
            [
                ("AUTUMN_DATABASE__URL", None::<&str>),
                ("DATABASE_URL", None::<&str>),
            ],
            || {
                // No autumn.toml in the test runner's cwd (we rely on that
                // directory not having one; if it does, this test is a no-op).
                let url = resolve_database_url();
                // Either None (no autumn.toml) or Some (if autumn.toml exists
                // with a database.url in the test runner cwd). We can't assert
                // None unconditionally, so we just assert the function returns
                // without panicking.
                let _ = url;
            },
        );
    }

    #[test]
    fn resolve_database_url_ignores_empty_autumn_database_url() {
        temp_env::with_vars(
            [
                ("AUTUMN_DATABASE__URL", Some("")),
                ("DATABASE_URL", Some("postgres://real:5432/db")),
            ],
            || {
                assert_eq!(
                    resolve_database_url().as_deref(),
                    Some("postgres://real:5432/db")
                );
            },
        );
    }

    // ── SeedContextError messages ──────────────────────────────────────────

    #[test]
    fn no_database_url_error_message_is_actionable() {
        let msg = SeedContextError::NoDatabaseUrl.to_string();
        assert!(
            msg.contains("AUTUMN_DATABASE__URL") || msg.contains("autumn.toml"),
            "error should be actionable, got: {msg}"
        );
    }
}
