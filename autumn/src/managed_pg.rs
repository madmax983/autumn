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

/// The managed provider that most recently started a cluster this process,
/// retained so abnormal boot-failure paths can stop the supervised child even
/// though they bypass `on_shutdown`. Last-writer-wins for the same reason.
static EMERGENCY_PROVIDER: std::sync::Mutex<Option<ManagedPostgresPoolProvider>> =
    std::sync::Mutex::new(None);

/// Synchronously stop the managed Postgres cluster started this process, if any.
///
/// Wired into abnormal boot-failure paths (e.g. a failed startup migration) that
/// call [`std::process::exit`] — which skips `on_shutdown` hooks and Rust
/// destructors and would otherwise orphan the supervised Postgres child while it
/// holds the data dir and port. Best-effort and idempotent: a no-op when no
/// managed cluster is running or it was already stopped cleanly.
pub(crate) fn emergency_stop() {
    let Some(provider) = EMERGENCY_PROVIDER.lock().ok().and_then(|g| g.clone()) else {
        return;
    };
    // These exit paths run on a blocking migration thread (no async runtime
    // entered), so drive the async stop to completion on a short-lived
    // current-thread runtime.
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt.block_on(provider.stop()),
        Err(e) => {
            tracing::warn!(error = %e, "could not stop managed Postgres during emergency shutdown");
        }
    }
}

/// Async variant of [`emergency_stop`] for boot-failure paths that run *inside*
/// the async runtime (e.g. a socket-bind failure after the cluster started),
/// where [`emergency_stop`]'s nested blocking runtime would panic.
pub(crate) async fn emergency_stop_async() {
    let provider = EMERGENCY_PROVIDER.lock().ok().and_then(|g| g.clone());
    if let Some(provider) = provider {
        provider.stop().await;
    }
}

/// Generate a random 24-character alphanumeric password for the managed cluster.
fn generate_password() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 24];
    // Fall back to a time-seeded value only if the OS RNG is unavailable; the
    // password protects a local single-user cluster.
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0u128, |d| d.as_nanos());
        let seed = nanos.to_le_bytes();
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = seed[i % seed.len()];
        }
    }
    bytes
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect()
}

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

    /// Directory the bundled/downloaded Postgres binaries are extracted to, kept
    /// under the managed data root (the data dir's parent) so it never depends on
    /// `$HOME`.
    fn installation_dir(&self) -> PathBuf {
        self.data_dir
            .parent()
            .unwrap_or(&self.data_dir)
            .join("postgresql")
    }

    /// Provision (if needed) and start the cluster, returning its connection URL.
    async fn ensure_running(&self) -> Result<String, postgresql_embedded::Error> {
        // Already started (e.g. a second pool, or repeated calls): reuse the
        // running instance instead of trying to start a second server on the
        // same — now locked — data dir.
        if let Ok(guard) = self.instance.lock()
            && let Some(pg) = guard.as_ref()
        {
            return Ok(pg.settings().url(MANAGED_DB_NAME));
        }

        let mut settings = Settings::new();
        settings.data_dir.clone_from(&self.data_dir);
        // Extract the Postgres binaries under the managed data root rather than
        // `Settings::new()`'s `$HOME/.theseus` default, which may be unset or
        // unwritable under launchd/systemd/container supervisors and would fail
        // before `initdb` even when the CLI-provided data dir is valid.
        settings.installation_dir = self.installation_dir();
        settings.version = self.version.clone();
        // Persistent cluster tied to the daemon, not a throwaway temp dir.
        settings.temporary = false;
        settings.timeout = Some(STARTUP_TIMEOUT);
        // Stable superuser credentials: `initdb` bakes the password into the
        // cluster on first boot only, but `Settings::new()` generates a *fresh*
        // random password (and a temp password file) every time. Persisting
        // them next to the data dir keeps later boots able to authenticate.
        let (password, password_file) = self.stable_credentials();
        settings.password = password;
        settings.password_file = password_file;

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

        // Register for emergency shutdown so an abnormal boot exit (which
        // bypasses `on_shutdown`) still stops the supervised child instead of
        // orphaning it. The clone shares `instance`, so this is idempotent with
        // a normal `stop()`.
        if let Ok(mut g) = EMERGENCY_PROVIDER.lock() {
            *g = Some(self.clone());
        }

        Ok(url)
    }

    /// Stable superuser password + password-file path persisted alongside the
    /// cluster (in the data dir's parent, since `initdb` requires an empty data
    /// dir). Generated once on first boot and reused on every later boot.
    fn stable_credentials(&self) -> (String, PathBuf) {
        let parent = self
            .data_dir
            .parent()
            .unwrap_or(&self.data_dir)
            .to_path_buf();
        let _ = std::fs::create_dir_all(&parent);
        let password_file = parent.join(".autumn-pg-superuser");

        if let Ok(existing) = std::fs::read_to_string(&password_file) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                // An existing file (older build, manual creation) may have a
                // permissive mode; the create-time `0600` below never applied to
                // it. Tighten it before reusing — it holds the cluster superuser
                // password.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &password_file,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
                return (trimmed.to_owned(), password_file);
            }
        }

        let password = generate_password();
        // 0600 — the password grants superuser on the local cluster.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if let Ok(mut f) = options.open(&password_file) {
            use std::io::Write;
            // `mode(0o600)` only applies when the file is newly created; an
            // existing empty/permissive file (interrupted first boot, manual
            // `touch`) keeps its old mode, so tighten it before writing the
            // superuser password.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
            }
            let _ = f.write_all(password.as_bytes());
        }
        (password, password_file)
    }

    /// Stop the supervised Postgres child. Wire this to the daemon's
    /// `on_shutdown` so the cluster shuts down with the app.
    pub async fn stop(&self) {
        let taken = self.instance.lock().ok().and_then(|mut guard| guard.take());
        if let Some(pg) = taken
            && let Err(e) = pg.stop().await
        {
            tracing::warn!(error = %e, "failed to stop managed Postgres cleanly");
        }
        // Clear the emergency-stop registration so a later app in the same
        // process (integration tests) doesn't try to emergency-stop this
        // now-dead handle. The resolved URL is carried on the per-app topology,
        // so there is no process-global URL to clear.
        if let Ok(mut g) = EMERGENCY_PROVIDER.lock() {
            *g = None;
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

    async fn create_topology(
        &self,
        config: &DatabaseConfig,
    ) -> Result<Option<crate::db::DatabaseTopology>, PoolError> {
        // The managed cluster is a single local primary. Ignore any external
        // `replica_url` from the original config so reads don't silently hit an
        // unrelated/stale replica while writes and migrations go to the managed
        // primary; the default impl would build that replica pool.
        let Some(primary) = self.create_pool(config).await? else {
            return Ok(None);
        };
        // After `create_pool`, the cluster is running; read its URL from the
        // retained handle and carry it on the topology so startup migrations
        // target this managed cluster — per-app, with no process-global.
        let migration_url = self
            .instance
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|pg| pg.settings().url(MANAGED_DB_NAME)));
        Ok(Some(
            crate::db::DatabaseTopology::from_pools(primary, None)
                .with_migration_url(migration_url),
        ))
    }

    async fn create_shard_topology(
        &self,
        _shard: &crate::config::ShardConfig,
        _defaults: &DatabaseConfig,
    ) -> Result<crate::db::DatabaseTopology, PoolError> {
        // The managed cluster is a single local Postgres; it can't back the
        // separate per-shard databases `[[database.shards]]` requires. The trait
        // default would point each shard at its external URL while the control
        // plane and migrations use the managed DB — a silent split. Fail fast
        // with a clear diagnostic instead (matching `create_pool`'s fatal-boot
        // handling).
        tracing::error!(
            "managed Postgres does not support [[database.shards]]; remove the \
             shard configuration or use an external database"
        );
        eprintln!(
            "autumn: managed Postgres (--bundled-pg) does not support \
             [[database.shards]]. Remove shard config or use an external database."
        );
        // `setup_database` already started the cluster via `create_topology`;
        // `process::exit` skips `on_shutdown`, so stop the child first to avoid
        // orphaning it on the data dir/port.
        self.stop().await;
        std::process::exit(1);
    }
}

/// Resolve the cluster data dir: the `AUTUMN_MANAGED_PG_DATA_DIR` override
/// (set per-project by `autumn serve`), then a per-user XDG-style default, then a
/// temp-dir fallback. The default/fallback are namespaced per project so two
/// unrelated apps using the provider directly (e.g. `cargo run`) don't share —
/// and contend for — one cluster.
fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(MANAGED_PG_DATA_DIR_ENV) {
        return PathBuf::from(dir);
    }
    let project = project_data_namespace();
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local/share/autumn/pg")
            .join(project);
    }
    // Windows (where HOME is usually unset): use a persistent app-data dir
    // rather than the volatile temp dir, which is cleared on reboot.
    if let Some(local_appdata) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_appdata)
            .join("autumn")
            .join("pg")
            .join(project);
    }
    std::env::temp_dir().join("autumn").join("pg").join(project)
}

/// A per-project namespace component for the fallback data dir, derived from the
/// app's manifest dir hashed with SHA-256. Stable across runs and toolchains so
/// an app keeps finding its cluster.
///
/// `#[autumn_web::main]` records the compile-time manifest dir via
/// `__set_macro_context` (not an exported env var), so resolve it through
/// [`config::OsEnv`], which surfaces that baked value for `AUTUMN_MANIFEST_DIR` —
/// otherwise installed binaries run from a common working dir would all hash the
/// same CWD and share one cluster. Falls back to the CWD only when no manifest
/// context exists.
fn project_data_namespace() -> String {
    use crate::config::Env;
    use sha2::{Digest, Sha256};
    let base = crate::config::OsEnv
        .var("AUTUMN_MANIFEST_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .and_then(|d| std::fs::canonicalize(&d).ok().or(Some(d)))
        .unwrap_or_else(|| PathBuf::from("."));
    let digest = Sha256::digest(base.to_string_lossy().as_bytes());
    hex::encode(&digest[..4])
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
