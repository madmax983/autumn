//! Managed local Postgres.
//!
//! A [`DatabasePoolProvider`] that provisions and supervises a local `postgres`
//! child in the app's data dir, surfaced through the existing pluggable
//! pool-provider seam (no changes to the query path).
//!
//! Enabled by the `managed-pg` feature. With `managed-pg-bundled` the Postgres
//! binaries are embedded in the app executable; otherwise they are downloaded
//! on first run. Either way Postgres runs as a supervised child process with an
//! on-disk data dir (it is never linked in-process).
//!
//! # Example
//!
//! ```ignore
//! let pg = ManagedPostgresPoolProvider::new();
//! autumn_web::app()
//!     .with_pool_provider(pg.clone())
//!     .on_shutdown(move || { let pg = pg.clone(); async move { pg.stop().await; } })
//!     .run()
//!     .await;
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::deadpool::Pool;
use postgresql_embedded::{PostgreSQL, Settings, VersionReq};

use crate::config::DatabaseConfig;
use crate::db::{DatabasePoolProvider, PoolError, create_pool};

/// Database created inside the managed cluster for the application.
const MANAGED_DB_NAME: &str = "autumn";

/// Environment variable that points the provider at a persistent data dir.
/// `autumn serve` sets this to a platform data dir; falls back to a per-user
/// default otherwise.
pub const MANAGED_PG_DATA_DIR_ENV: &str = "AUTUMN_MANAGED_PG_DATA_DIR";

/// How long to allow each `initdb`/`start` step before surfacing a diagnostic.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// A [`DatabasePoolProvider`] backed by a managed, locally-supervised Postgres.
///
/// `create_pool` runs `initdb` (idempotent), starts the server, ensures the
/// application database exists, and then builds the normal `deadpool` pool from
/// the resulting URL — reusing all of Autumn's existing pool logic. The running
/// server handle is retained so the child is not stopped when the call returns;
/// [`stop`](Self::stop) (wired to the daemon's `on_shutdown`) shuts it down.
#[derive(Clone)]
pub struct ManagedPostgresPoolProvider {
    data_dir: PathBuf,
    version: VersionReq,
    instance: Arc<Mutex<Option<PostgreSQL>>>,
}

impl std::fmt::Debug for ManagedPostgresPoolProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedPostgresPoolProvider")
            .field("data_dir", &self.data_dir)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Default for ManagedPostgresPoolProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ManagedPostgresPoolProvider {
    /// Construct a provider using the data dir from `AUTUMN_MANAGED_PG_DATA_DIR`
    /// (set by `autumn serve`) or a per-user default.
    #[must_use]
    pub fn new() -> Self {
        Self::with_data_dir(default_data_dir())
    }

    /// Construct a provider that stores its cluster under `data_dir`.
    #[must_use]
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            version: postgresql_embedded::LATEST.clone(),
            instance: Arc::new(Mutex::new(None)),
        }
    }

    /// Provision (if needed) and start the cluster, returning its connection URL.
    async fn ensure_running(&self) -> Result<String, postgresql_embedded::Error> {
        let mut settings = Settings::new();
        settings.data_dir.clone_from(&self.data_dir);
        settings.version = self.version.clone();
        // Persistent cluster tied to the daemon, not a throwaway temp dir.
        settings.temporary = false;
        settings.timeout = Some(STARTUP_TIMEOUT);

        let mut pg = PostgreSQL::new(settings);
        // `setup` is idempotent: it downloads/extracts and runs `initdb` only
        // when the data dir is not already initialized.
        pg.setup().await?;
        pg.start().await?;
        if !pg.database_exists(MANAGED_DB_NAME).await? {
            pg.create_database(MANAGED_DB_NAME).await?;
        }
        let url = pg.settings().url(MANAGED_DB_NAME);

        // Retain the handle so the child keeps running after this returns.
        if let Ok(mut guard) = self.instance.lock() {
            *guard = Some(pg);
        }
        Ok(url)
    }

    /// Stop the supervised Postgres child. Wire this to the daemon's
    /// `on_shutdown` so the cluster shuts down with the app.
    pub async fn stop(&self) {
        let taken = self
            .instance
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(pg) = taken
            && let Err(e) = pg.stop().await
        {
            tracing::warn!(error = %e, "failed to stop managed Postgres cleanly");
        }
    }
}

impl DatabasePoolProvider for ManagedPostgresPoolProvider {
    async fn create_pool(
        &self,
        config: &DatabaseConfig,
    ) -> Result<Option<Pool<AsyncPgConnection>>, PoolError> {
        let url = match self.ensure_running().await {
            Ok(url) => url,
            Err(e) => {
                // A failed cluster start is fatal at boot — surface a clear
                // diagnostic instead of hanging or silently running DB-less.
                tracing::error!(error = %e, data_dir = %self.data_dir.display(),
                    "managed Postgres failed to provision/start");
                eprintln!(
                    "autumn: managed Postgres failed to start ({e}). \
                     Data dir: {}",
                    self.data_dir.display()
                );
                std::process::exit(1);
            }
        };

        // Build the normal pool from the managed URL; reuses all pool logic.
        let mut managed = config.clone();
        managed.primary_url = Some(url);
        managed.url = None;
        create_pool(&managed)
    }
}

/// Resolve the cluster data dir: the `AUTUMN_MANAGED_PG_DATA_DIR` override, then
/// a per-user XDG-style default, then a temp-dir fallback.
fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(MANAGED_PG_DATA_DIR_ENV) {
        return PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/autumn/pg");
    }
    std::env::temp_dir().join("autumn").join("pg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_data_dir_honors_env_override() {
        temp_env::with_var(MANAGED_PG_DATA_DIR_ENV, Some("/tmp/custom/pg"), || {
            assert_eq!(default_data_dir(), PathBuf::from("/tmp/custom/pg"));
        });
    }

    #[test]
    fn with_data_dir_is_used() {
        let p = ManagedPostgresPoolProvider::with_data_dir(PathBuf::from("/var/lib/x"));
        assert_eq!(p.data_dir, PathBuf::from("/var/lib/x"));
    }
}
