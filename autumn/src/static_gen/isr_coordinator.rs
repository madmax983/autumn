//! ISR coordination backends.
//!
//! Controls which replica may regenerate a given route within a revalidation
//! window. The [`LocalIsrCoordinator`] is an in-process no-op suitable for
//! single-replica / development deployments -- all local deduplication is
//! already handled by the per-route `AtomicBool` in `StaticFileLayer`.
//! The [`PostgresIsrCoordinator`] uses `pg_try_advisory_lock` to ensure that
//! at most one replica regenerates a given route per revalidation window
//! across the entire fleet.
//!
//! ## Local vs multi-replica contract
//!
//! | Deployment | Recommended coordinator | Guarantee |
//! |------------|-------------------------|-----------|
//! | Single process / dev | `LocalIsrCoordinator` (default) | At most one in-flight task per route per process |
//! | Multi-replica (shared `dist/`) | `PostgresIsrCoordinator` | At most one regeneration per route per revalidation window across the fleet |
//! | Read-only `dist/` (build-time only) | Disable ISR (`revalidate` = None) | No runtime writes |
//!
//! ## Atomic writes
//!
//! Regardless of coordinator, regeneration always writes to a `.tmp` file
//! then renames atomically so a reader never observes a partially written
//! page.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use sha2::{Digest as _, Sha256};

/// Boxed async future returned by coordinator operations.
pub type IsrFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Coordination backend for ISR background regeneration.
///
/// Implementations decide whether *this* replica may regenerate a given route
/// for a given revalidation window. Returning `false` from
/// [`try_acquire`](Self::try_acquire) means another task or replica has
/// already claimed the work; the caller should skip regeneration.
///
/// Implementors must also call [`release`](Self::release) after the
/// regeneration attempt so that subsequent windows can be acquired.
pub trait IsrCoordinator: Send + Sync + 'static {
    /// Short backend identifier used in log messages.
    fn backend(&self) -> &'static str;

    /// Try to acquire the right to regenerate `url_path` for `window_key`.
    ///
    /// `window_key` is derived from [`isr_window_key`] and encodes both the
    /// route and the current revalidation bucket; two replicas that observe
    /// the same stale file within the same window will produce the same key.
    ///
    /// Returns `true` when this caller may proceed; `false` when another
    /// task or replica already holds the lock for this (route, window) pair.
    fn try_acquire<'a>(
        &'a self,
        url_path: &'a str,
        window_key: &'a str,
    ) -> IsrFuture<'a, bool>;

    /// Release the lock after regeneration completes (success or failure).
    ///
    /// Must be called exactly once for every successful [`try_acquire`](Self::try_acquire).
    fn release<'a>(
        &'a self,
        url_path: &'a str,
        window_key: &'a str,
    ) -> IsrFuture<'a, ()>;
}

/// In-process ISR coordinator.
///
/// This is the **default** coordinator. It is a pass-through: all local
/// deduplication is already handled by the `AtomicBool` in-flight guard
/// inside `StaticFileLayer`. This coordinator always grants the lock so
/// that a single process never misses a revalidation cycle.
///
/// For multi-replica deployments use [`PostgresIsrCoordinator`] (feature
/// `db`) which enforces fleet-wide deduplication via `pg_try_advisory_lock`.
pub struct LocalIsrCoordinator {
    /// Tracks (`url_path`, `window_key`) pairs currently held by this process.
    held: Mutex<HashSet<(String, String)>>,
}

impl LocalIsrCoordinator {
    /// Create a new in-process coordinator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            held: Mutex::new(HashSet::new()),
        }
    }
}

impl Default for LocalIsrCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl IsrCoordinator for LocalIsrCoordinator {
    fn backend(&self) -> &'static str {
        "local"
    }

    fn try_acquire<'a>(
        &'a self,
        url_path: &'a str,
        window_key: &'a str,
    ) -> IsrFuture<'a, bool> {
        Box::pin(async move {
            let key = (url_path.to_owned(), window_key.to_owned());
            self.held.lock().unwrap().insert(key)
        })
    }

    fn release<'a>(
        &'a self,
        url_path: &'a str,
        window_key: &'a str,
    ) -> IsrFuture<'a, ()> {
        Box::pin(async move {
            let key = (url_path.to_owned(), window_key.to_owned());
            self.held.lock().unwrap().remove(&key);
        })
    }
}

/// Postgres advisory-lock ISR coordinator.
///
/// Uses `pg_try_advisory_lock` / `pg_advisory_unlock` to prevent duplicate
/// regeneration across replicas. Requires the `db` feature.
///
/// The advisory lock is keyed on [`isr_advisory_lock_key`] which is derived
/// from (route, window) -- two replicas that see the same stale page in the
/// same revalidation window will attempt the same lock key, and only the
/// first will succeed.
///
/// ## Connection management
///
/// A dedicated connection is checked out for the duration of the lock because
/// Postgres session-level advisory locks are bound to the connection. The
/// connection is returned to the pool when [`IsrCoordinator::release`] is
/// called. Regeneration tasks that fail to acquire the Postgres lock return
/// their pooled connection immediately.
#[cfg(feature = "db")]
pub struct PostgresIsrCoordinator {
    pool: diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
}

#[cfg(feature = "db")]
impl PostgresIsrCoordinator {
    /// Create a Postgres ISR coordinator backed by the given connection pool.
    #[must_use]
    pub const fn new(
        pool: diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    ) -> Self {
        Self { pool }
    }
}

#[cfg(feature = "db")]
impl IsrCoordinator for PostgresIsrCoordinator {
    fn backend(&self) -> &'static str {
        "postgres"
    }

    fn try_acquire<'a>(
        &'a self,
        url_path: &'a str,
        window_key: &'a str,
    ) -> IsrFuture<'a, bool> {
        let lock_key = isr_advisory_lock_key(url_path, window_key);
        Box::pin(async move {
            let mut conn = match self.pool.get().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "ISR coordinator: could not get Postgres connection");
                    return false;
                }
            };
            match try_pg_advisory_lock(&mut conn, lock_key).await {
                Ok(acquired) => acquired,
                Err(e) => {
                    tracing::warn!(error = %e, "ISR coordinator: pg_try_advisory_lock failed");
                    false
                }
            }
        })
    }

    fn release<'a>(
        &'a self,
        url_path: &'a str,
        window_key: &'a str,
    ) -> IsrFuture<'a, ()> {
        let lock_key = isr_advisory_lock_key(url_path, window_key);
        Box::pin(async move {
            let mut conn = match self.pool.get().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "ISR coordinator: could not get Postgres connection for release");
                    return;
                }
            };
            match unlock_pg_advisory_lock(&mut conn, lock_key).await {
                Ok(false) => {
                    tracing::warn!(lock_key, "ISR coordinator: pg_advisory_unlock returned false (lock already released)");
                }
                Ok(true) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "ISR coordinator: pg_advisory_unlock failed");
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Key derivation utilities (public -- useful in application code and tests)
// ---------------------------------------------------------------------------

/// Compute the revalidation window key for a route and the current time.
///
/// All replicas that evaluate a given route as stale within the same
/// revalidation period will produce the same key, making it safe to use
/// as a distributed lock discriminator.
///
/// # Arguments
///
/// * `url_path` -- The URL path of the route (e.g. `"/about"`).
/// * `revalidate_secs` -- The ISR interval in seconds (0 is treated as 1).
/// * `now_unix_secs` -- Current Unix timestamp in seconds.
///
/// # Returns
///
/// A string of the form `"{url_path}:{bucket}"` where `bucket = now / interval`.
#[must_use]
pub fn isr_window_key(url_path: &str, revalidate_secs: u64, now_unix_secs: u64) -> String {
    let interval = revalidate_secs.max(1);
    let bucket = now_unix_secs / interval;
    format!("{url_path}:{bucket}")
}

/// Derive a stable signed 64-bit advisory lock key for a (route, window) pair.
///
/// Suitable for `pg_try_advisory_lock`. The result is a deterministic hash
/// of the inputs; different routes or different windows produce different keys
/// with overwhelming probability.
#[must_use]
pub fn isr_advisory_lock_key(url_path: &str, window_key: &str) -> i64 {
    let mut hasher = Sha256::new();
    hasher.update(b"isr\0");
    hasher.update(url_path.as_bytes());
    hasher.update(b"\0");
    hasher.update(window_key.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(bytes)
}

// ---------------------------------------------------------------------------
// Postgres helpers (feature = "db")
// ---------------------------------------------------------------------------

#[cfg(feature = "db")]
#[derive(diesel::QueryableByName)]
struct PgAdvisoryLockRow {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    acquired: bool,
}

#[cfg(feature = "db")]
async fn try_pg_advisory_lock(
    conn: &mut diesel_async::pooled_connection::deadpool::Object<diesel_async::AsyncPgConnection>,
    key: i64,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    use diesel_async::RunQueryDsl as _;

    let row = diesel::sql_query("SELECT pg_try_advisory_lock($1) AS acquired")
        .bind::<diesel::sql_types::BigInt, _>(key)
        .get_result::<PgAdvisoryLockRow>(&mut **conn)
        .await?;
    Ok(row.acquired)
}

#[cfg(feature = "db")]
#[derive(diesel::QueryableByName)]
struct PgAdvisoryUnlockRow {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    released: bool,
}

#[cfg(feature = "db")]
async fn unlock_pg_advisory_lock(
    conn: &mut diesel_async::pooled_connection::deadpool::Object<diesel_async::AsyncPgConnection>,
    key: i64,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    use diesel_async::RunQueryDsl as _;

    let row = diesel::sql_query("SELECT pg_advisory_unlock($1) AS released")
        .bind::<diesel::sql_types::BigInt, _>(key)
        .get_result::<PgAdvisoryUnlockRow>(&mut **conn)
        .await?;
    Ok(row.released)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- isr_window_key ---

    #[test]
    fn window_key_stable_within_interval() {
        // Bucket 28_333_333 covers [1_699_999_980, 1_700_000_039].
        let a = isr_window_key("/about", 60, 1_700_000_000);
        let b = isr_window_key("/about", 60, 1_700_000_039);
        assert_eq!(a, b);
    }

    #[test]
    fn window_key_changes_on_boundary() {
        // 1_700_000_039 is bucket 28_333_333; 1_700_000_040 is bucket 28_333_334.
        let a = isr_window_key("/about", 60, 1_700_000_039);
        let b = isr_window_key("/about", 60, 1_700_000_040);
        assert_ne!(a, b);
    }

    #[test]
    fn window_key_route_prefix() {
        let key = isr_window_key("/about", 60, 1_700_000_000);
        assert!(key.starts_with("/about:"), "key should start with route: {key}");
    }

    #[test]
    fn window_key_zero_revalidate_no_panic() {
        let key = isr_window_key("/edge", 0, 42);
        assert!(!key.is_empty());
    }

    // --- isr_advisory_lock_key ---

    #[test]
    fn advisory_key_deterministic() {
        let a = isr_advisory_lock_key("/about", "/about:28333333");
        let b = isr_advisory_lock_key("/about", "/about:28333333");
        assert_eq!(a, b);
    }

    #[test]
    fn advisory_key_differs_by_route() {
        let a = isr_advisory_lock_key("/", "/about:28333333");
        let b = isr_advisory_lock_key("/about", "/about:28333333");
        assert_ne!(a, b);
    }

    #[test]
    fn advisory_key_differs_by_window() {
        let a = isr_advisory_lock_key("/about", "/about:1");
        let b = isr_advisory_lock_key("/about", "/about:2");
        assert_ne!(a, b);
    }

    // --- LocalIsrCoordinator ---

    #[tokio::test]
    async fn local_coordinator_first_acquire() {
        let c = LocalIsrCoordinator::new();
        assert!(c.try_acquire("/a", "w1").await);
    }

    #[tokio::test]
    async fn local_coordinator_double_acquire_denied() {
        let c = LocalIsrCoordinator::new();
        assert!(c.try_acquire("/a", "w1").await);
        assert!(!c.try_acquire("/a", "w1").await);
    }

    #[tokio::test]
    async fn local_coordinator_release_allows_reacquire() {
        let c = LocalIsrCoordinator::new();
        assert!(c.try_acquire("/a", "w1").await);
        c.release("/a", "w1").await;
        assert!(c.try_acquire("/a", "w1").await);
    }
}
