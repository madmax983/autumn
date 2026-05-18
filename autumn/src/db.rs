//! Database connection pool and extractor.
//!
//! This module provides async Postgres connectivity via `diesel-async` with
//! the `deadpool` connection pool. The pool is created at startup by
//! [`AppBuilder::run`](crate::app::AppBuilder::run) and stored in
//! [`crate::state::AppState`].
//!
//! When no `database.primary_url` or legacy `database.url` is configured,
//! [`create_pool`] returns `Ok(None)` and the application runs without a
//! database -- useful for static-site or API-gateway use cases.
//!
//! # The [`Db`] extractor
//!
//! Declare `db: Db` in your handler signature to get a pooled connection.
//! The connection is automatically returned to the pool when `Db` is dropped
//! at the end of the request.
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//!
//! #[get("/hello")]
//! async fn hello(db: Db) -> AutumnResult<String> {
//!     // Use `db` with Diesel queries...
//!     Ok("hello from db".to_string())
//! }
//! ```

use axum::extract::FromRequestParts;
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;
use futures::FutureExt as _;
use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::Instrument as _;

use crate::config::DatabaseConfig;
use crate::error::AutumnError;

// ── After-commit callback infrastructure ─────────────────────────────────────

/// A boxed async callback registered for post-transaction execution.
///
/// Stored in [`AFTER_COMMIT_REGISTRY`] during an active [`Db::tx`] block.
/// The registry is drained and each callback is awaited after the transaction
/// commits successfully. On rollback or panic the callbacks are dropped
/// without being called.
pub type CommitCallback = Box<
    dyn FnOnce() -> Pin<Box<dyn Future<Output = crate::AutumnResult<()>> + Send + 'static>>
        + Send
        + 'static,
>;

tokio::task_local! {
    /// Task-local registry used by [`Db::tx`] to accumulate after-commit
    /// callbacks. Only set while the [`Db::tx`] future is being polled;
    /// absent outside a transaction block.
    pub static AFTER_COMMIT_REGISTRY: Arc<Mutex<Vec<CommitCallback>>>;
}

/// Total count of after-commit callback errors since process start.
///
/// Incremented each time a callback registered via [`register_after_commit`]
/// or [`Db::tx`] returns an error **after** the transaction has already
/// committed. The underlying transaction is unaffected; this counter surfaces
/// failures for alerting and dashboards.
///
/// Exposed by the `/actuator/health` endpoint as the top-level
/// `autumn_after_commit_failures_total` field.
pub static AFTER_COMMIT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn record_after_commit_failure() -> u64 {
    AFTER_COMMIT_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed) + 1
}

pub(crate) fn reject_ambient_after_commit_registry_for_tx() -> Result<(), AutumnError> {
    if AFTER_COMMIT_REGISTRY.try_with(|_| ()).is_ok() {
        return Err(AutumnError::bad_request_msg(
            "Nested Db::tx calls are not supported",
        ));
    }
    Ok(())
}

pub(crate) fn spawn_committed_after_commit_callbacks(
    callbacks: Vec<CommitCallback>,
) -> Option<tokio::task::JoinHandle<()>> {
    if callbacks.is_empty() {
        return None;
    }

    Some(tokio::task::spawn(async move {
        for cb in callbacks {
            let result = match std::panic::catch_unwind(AssertUnwindSafe(cb)) {
                Ok(callback) => AssertUnwindSafe(callback).catch_unwind().await,
                Err(panic) => Err(panic),
            };

            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let failures_total = record_after_commit_failure();
                    tracing::error!(
                        autumn.after_commit.failures_total = failures_total,
                        "after_commit callback failed (tx already committed): {e}"
                    );
                }
                Err(panic) => {
                    let failures_total = record_after_commit_failure();
                    let panic = after_commit_panic_message(&*panic);
                    tracing::error!(
                        autumn.after_commit.failures_total = failures_total,
                        "after_commit callback panicked (tx already committed): {panic}"
                    );
                }
            }
        }
    }))
}

fn after_commit_panic_message(payload: &(dyn Any + Send)) -> String {
    match (
        payload.downcast_ref::<&'static str>(),
        payload.downcast_ref::<String>(),
    ) {
        (Some(message), _) => (*message).to_owned(),
        (_, Some(message)) => message.clone(),
        (None, None) => "non-string panic payload".to_owned(),
    }
}

/// Register a callback to run after the current database transaction commits.
///
/// If called inside a [`Db::tx`] block, the callback is deferred until the
/// transaction commits successfully. On rollback the callback is dropped
/// without being called.
///
/// The deferred callback is process-local work spawned after commit. It avoids
/// side effects for rolled-back transactions, but it is not a crash-safe
/// delivery mechanism. For side effects that must survive process exit, write a
/// durable outbox or queue row inside the same database transaction and use
/// this callback only as an optional wake-up hint.
///
/// If called **outside** any active transaction, the callback runs immediately
/// (eager execution) with a `debug`-level log note.
///
/// # Panics
///
/// Panics if the internal registry mutex is poisoned (only possible if a
/// previous thread holding the lock panicked, which should not occur in normal
/// operation).
///
/// # Example
///
/// ```rust,ignore
/// db.tx(move |conn| {
///     scoped_boxed(async move {
///         diesel::insert_into(users::table).values(&new_user).execute(conn).await?;
///         autumn_web::db::register_after_commit(|| async {
///             welcome_email_job.enqueue("user_id", user_id).await
///         }).await;
///         Ok(())
///     })
/// }).await?;
/// ```
pub async fn register_after_commit<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = crate::AutumnResult<()>> + Send + 'static,
{
    let mut f_opt = Some(f);
    AFTER_COMMIT_REGISTRY
        .try_with(|registry| {
            let f = f_opt.take().expect("closure only entered once");
            let boxed: CommitCallback = Box::new(move || Box::pin(f()));
            registry.lock().expect("registry lock").push(boxed);
        })
        .ok();

    // If still Some, the task-local wasn't set — we're outside a tx; run eagerly.
    if let Some(f) = f_opt {
        tracing::debug!("register_after_commit: no active transaction; running callback eagerly");
        if let Err(e) = f().await {
            let failures_total = record_after_commit_failure();
            tracing::error!(
                autumn.after_commit.failures_total = failures_total,
                "register_after_commit eager callback failed: {e}"
            );
        }
    }
}

/// Trait to abstract the state requirement for the `Db` extractor.
/// This breaks the circular dependency between the database extractor
/// and the central `AppState`.
pub trait DbState {
    /// Returns the database connection pool, if configured.
    fn pool(&self) -> Option<&Pool<AsyncPgConnection>>;

    /// Returns the read/replica connection pool, if configured.
    fn replica_pool(&self) -> Option<&Pool<AsyncPgConnection>> {
        None
    }

    /// Returns the pool used for read-only work.
    ///
    /// Defaults to the replica role when present, otherwise the primary role.
    fn read_pool(&self) -> Option<&Pool<AsyncPgConnection>> {
        self.replica_pool().or_else(|| self.pool())
    }
}

/// Error type for pool creation failures.
///
/// Alias for the deadpool `BuildError`. Returned by [`create_pool`] when
/// the pool cannot be constructed (e.g., invalid max-size configuration).
pub type PoolError = diesel_async::pooled_connection::deadpool::BuildError;

/// Primary plus optional read-replica database pools.
#[derive(Clone)]
pub struct DatabaseTopology {
    primary: Pool<AsyncPgConnection>,
    replica: Option<Pool<AsyncPgConnection>>,
}

impl DatabaseTopology {
    /// Build a topology from explicit primary and optional replica pools.
    ///
    /// This is useful for custom [`DatabasePoolProvider`] implementations that
    /// need to create or decorate both roles themselves.
    #[must_use]
    pub const fn from_pools(
        primary: Pool<AsyncPgConnection>,
        replica: Option<Pool<AsyncPgConnection>>,
    ) -> Self {
        Self { primary, replica }
    }

    /// Build a topology from a primary pool only.
    #[must_use]
    pub const fn primary_only(primary: Pool<AsyncPgConnection>) -> Self {
        Self {
            primary,
            replica: None,
        }
    }

    /// Primary/write role pool.
    #[must_use]
    pub const fn primary(&self) -> &Pool<AsyncPgConnection> {
        &self.primary
    }

    /// Optional read/replica role pool.
    #[must_use]
    pub const fn replica(&self) -> Option<&Pool<AsyncPgConnection>> {
        self.replica.as_ref()
    }

    /// Pool used for read-only work.
    #[must_use]
    pub fn read(&self) -> &Pool<AsyncPgConnection> {
        self.replica.as_ref().unwrap_or(&self.primary)
    }
}

fn build_pool(
    url: &str,
    pool_size: usize,
    connect_timeout_secs: u64,
) -> Result<Pool<AsyncPgConnection>, PoolError> {
    let timeout = Duration::from_secs(connect_timeout_secs);
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
    Pool::builder(manager)
        .max_size(pool_size.max(1))
        .wait_timeout(Some(timeout))
        .create_timeout(Some(timeout))
        .runtime(deadpool::Runtime::Tokio1)
        .build()
}

/// Create a connection pool from the database configuration.
///
/// Returns `Ok(None)` if no primary database URL is configured
/// (`database.primary_url` and the legacy `database.url` are absent or `null`
/// in `autumn.toml`).
///
/// # Errors
///
/// Returns [`PoolError`] if the pool cannot be built (e.g., invalid
/// max-size configuration).
pub fn create_pool(config: &DatabaseConfig) -> Result<Option<Pool<AsyncPgConnection>>, PoolError> {
    let Some(url) = config.effective_primary_url() else {
        return Ok(None);
    };

    let pool = build_pool(
        url,
        config.effective_primary_pool_size(),
        config.connect_timeout_secs,
    )?;

    Ok(Some(pool))
}

/// Create primary and optional replica pools from the database configuration.
///
/// Returns `Ok(None)` when neither `database.primary_url` nor the legacy
/// `database.url` compatibility field is configured.
///
/// # Errors
///
/// Returns [`PoolError`] if either configured role cannot be built.
pub fn create_topology(config: &DatabaseConfig) -> Result<Option<DatabaseTopology>, PoolError> {
    let Some(primary_url) = config.effective_primary_url() else {
        return Ok(None);
    };

    let primary = build_pool(
        primary_url,
        config.effective_primary_pool_size(),
        config.connect_timeout_secs,
    )?;
    let replica = config
        .replica_url
        .as_deref()
        .map(|url| {
            build_pool(
                url,
                config.effective_replica_pool_size(),
                config.connect_timeout_secs,
            )
        })
        .transpose()?;

    Ok(Some(DatabaseTopology { primary, replica }))
}

// ── Db extractor ─────────────────────────────────────────────

/// Connection type managed by the deadpool pool.
type PooledConnection = diesel_async::pooled_connection::deadpool::Object<AsyncPgConnection>;

struct TxDepthGuard<'a> {
    depth: &'a mut usize,
    poisoned: &'a mut bool,
    disarmed: bool,
}

impl Drop for TxDepthGuard<'_> {
    fn drop(&mut self) {
        *self.depth -= 1;
        if !self.disarmed {
            *self.poisoned = true;
        }
    }
}

/// Async database connection extractor.
///
/// Declare `db: Db` in a handler signature to get a pooled connection to
/// Postgres. The connection is returned to the pool when `Db` is dropped
/// at the end of the request.
///
/// `Db` implements [`Deref`](std::ops::Deref) and
/// [`DerefMut`](std::ops::DerefMut) to
/// `diesel_async::AsyncPgConnection`, so you can use it directly with
/// Diesel query methods.
///
/// If no database is configured (i.e., `database.primary_url` and legacy
/// `database.url` are absent),
/// requests that use `Db` will receive a `503 Service Unavailable`
/// response.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/ping-db")]
/// async fn ping_db(db: Db) -> AutumnResult<&'static str> {
///     // `db` dereferences to AsyncPgConnection
///     Ok("database is reachable")
/// }
/// ```
pub struct Db {
    conn: PooledConnection,
    /// Span covering the full checkout-to-release window. Dropped when
    /// `Db` is dropped at the end of the request, so span duration
    /// reflects real connection hold time rather than just `pool.get()`
    /// latency. Exposed via [`Db::span`] so handlers can attach
    /// per-query spans as children with
    /// [`tracing::Instrument::instrument`].
    span: tracing::Span,
    tx_depth: usize,
    tx_poisoned: bool,
}

impl Db {
    /// Connection-scoped span. Instrument a query future with this to
    /// emit a child span tagged under the connection checkout window.
    ///
    /// ```rust,no_run
    /// use autumn_web::prelude::*;
    /// use tracing::Instrument as _;
    ///
    /// # async fn example(mut db: Db) -> AutumnResult<()> {
    /// let span = db.span().clone();
    /// // run a Diesel query here, e.g. users::table.load(&mut *db)
    /// async {
    ///     // ... diesel_async query ...
    ///     Ok::<_, AutumnError>(())
    /// }
    /// .instrument(span)
    /// .await
    /// # }
    /// ```
    #[must_use]
    pub const fn span(&self) -> &tracing::Span {
        &self.span
    }

    /// Run an async closure inside a database transaction.
    ///
    /// Commits when the closure returns `Ok(_)`, rolls back when it returns
    /// `Err(_)`.
    ///
    /// # Errors
    ///
    /// Returns [`AutumnError`] when:
    ///
    /// - the underlying transaction returns an error,
    /// - the closure returns an error that converts into `AutumnError`,
    /// - this `Db` is already inside a transaction,
    /// - this `Db` has been poisoned by a previously cancelled/dropped
    ///   transaction future.
    ///
    /// # Panics
    ///
    /// Panics if the internal after-commit registry mutex is poisoned (only
    /// possible if a previous thread holding the lock panicked).
    pub async fn tx<'a, T, E, F>(&'a mut self, f: F) -> Result<T, crate::error::AutumnError>
    where
        T: Send + 'a,
        E: From<diesel::result::Error> + Send + Sync + 'a,
        crate::error::AutumnError: From<E>,
        F: for<'r> FnOnce(
                &'r mut PooledConnection,
            ) -> scoped_futures::ScopedBoxFuture<'a, 'r, Result<T, E>>
            + Send
            + 'a,
    {
        use diesel_async::AsyncConnection as _;

        if self.tx_poisoned {
            return Err(crate::error::AutumnError::service_unavailable_msg(
                "Database connection is in an invalid transaction state",
            ));
        }
        if self.tx_depth > 0 {
            return Err(crate::error::AutumnError::bad_request_msg(
                "Nested Db::tx calls are not supported",
            ));
        }
        reject_ambient_after_commit_registry_for_tx()?;
        self.tx_depth += 1;
        let mut guard = TxDepthGuard {
            depth: &mut self.tx_depth,
            poisoned: &mut self.tx_poisoned,
            disarmed: false,
        };

        // Each tx gets its own callback registry shared with the task-local so
        // that code running inside the closure (jobs, mailer, hooks) can push
        // callbacks without having access to `Db` directly. The `Arc` lets us
        // read the registry after the `scope` future completes.
        let registry: Arc<Mutex<Vec<CommitCallback>>> = Arc::new(Mutex::new(Vec::new()));

        let result = AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), self.conn.transaction::<T, E, _>(f))
            .await
            .map_err(Into::into);

        guard.disarmed = true;

        // On commit: spawn the registered callbacks outside the transaction
        // connection, but await them sequentially inside that task so callback
        // dependencies observe registration order.
        // Errors are counted and logged; they do NOT affect the committed tx.
        if result.is_ok() {
            let callbacks: Vec<CommitCallback> = {
                let mut reg = registry.lock().expect("registry lock");
                std::mem::take(&mut *reg)
            };
            let _ = spawn_committed_after_commit_callbacks(callbacks);
        }

        result
    }
}

impl std::ops::Deref for Db {
    type Target = AsyncPgConnection;
    fn deref(&self) -> &Self::Target {
        assert!(
            !self.tx_poisoned,
            "Db connection is poisoned due to a cancelled/dropped transaction"
        );
        &self.conn
    }
}

impl std::ops::DerefMut for Db {
    fn deref_mut(&mut self) -> &mut Self::Target {
        assert!(
            !self.tx_poisoned,
            "Db connection is poisoned due to a cancelled/dropped transaction"
        );
        &mut self.conn
    }
}

impl<S> FromRequestParts<S> for Db
where
    S: DbState + Send + Sync,
{
    type Rejection = AutumnError;

    async fn from_request_parts(
        _parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let pool = state
            .pool()
            .ok_or_else(|| AutumnError::service_unavailable_msg("Database not configured"))?;

        // Span covers the full time the connection is held — from
        // checkout through the end of the request — rather than just
        // `pool.get()`. Dropping `Db` closes the span, so span duration
        // reflects real connection hold time and `db.system=postgresql`
        // propagates to any query futures handlers instrument with
        // `db.span()`.
        let span = tracing::info_span!(
            "db.connection",
            otel.kind = "client",
            db.system = "postgresql",
        );
        let conn = async {
            pool.get().await.map_err(|e| {
                tracing::error!("Failed to acquire database connection: {e}");
                AutumnError::service_unavailable_msg(e.to_string())
            })
        }
        .instrument(span.clone())
        .await?;

        Ok(Self {
            conn,
            span,
            tx_depth: 0,
            tx_poisoned: false,
        })
    }
}

// ----------------------------------------------------------------------------
// DatabasePoolProvider — tier-1 boot-time replaceable pool factory
// ----------------------------------------------------------------------------

/// Pluggable boot-time database pool factory.
///
/// Replace the default `deadpool + diesel-async` factory with a custom
/// strategy (custom metrics wrapper, circuit breaker, separate pools per
/// shard, etc.) by implementing this trait and installing it on the
/// [`AppBuilder`](crate::app::AppBuilder) via
/// [`with_pool_provider`](crate::app::AppBuilder::with_pool_provider).
///
/// The trait abstracts the *factory*, not the pool *type* — the return type is
/// fixed at `Pool<AsyncPgConnection>` for now. Swapping to a different backend
/// (e.g. `MySQL`, `SQLite`) would require generic `Pool<C>` propagation through
/// `Db` / `DbState` / `AppState` and is intentionally out of scope.
///
/// Providers that only implement [`DatabasePoolProvider::create_pool`] still
/// participate in primary/replica topology: the default
/// [`DatabasePoolProvider::create_topology`] uses the custom primary pool and
/// builds the configured replica role with Autumn's deadpool factory. Override
/// `create_topology` when both roles need custom construction.
///
/// # Example
///
/// ```rust,no_run
/// use autumn_web::config::DatabaseConfig;
/// use autumn_web::db::{DatabasePoolProvider, PoolError};
/// use diesel_async::AsyncPgConnection;
/// use diesel_async::pooled_connection::deadpool::Pool;
///
/// pub struct MetricsPoolProvider;
///
/// impl DatabasePoolProvider for MetricsPoolProvider {
///     async fn create_pool(
///         &self,
///         config: &DatabaseConfig,
///     ) -> Result<Option<Pool<AsyncPgConnection>>, PoolError> {
///         // Wrap the default pool with custom metrics, then return it.
///         autumn_web::db::create_pool(config)
///     }
/// }
/// ```
pub trait DatabasePoolProvider: Send + Sync + 'static {
    /// Create a connection pool from the resolved [`DatabaseConfig`].
    ///
    /// Returning `Ok(None)` signals that the application should run without a
    /// database — useful for static-site / API-gateway use cases or for
    /// disabling the DB in test contexts.
    fn create_pool(
        &self,
        config: &DatabaseConfig,
    ) -> impl std::future::Future<Output = Result<Option<Pool<AsyncPgConnection>>, PoolError>> + Send;

    /// Create primary and optional replica pools from the resolved
    /// [`DatabaseConfig`].
    ///
    /// The default implementation preserves the provider's custom primary pool
    /// and builds a replica pool when `database.replica_url` is configured.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError`] if either configured role cannot be built.
    fn create_topology(
        &self,
        config: &DatabaseConfig,
    ) -> impl std::future::Future<Output = Result<Option<DatabaseTopology>, PoolError>> + Send {
        async move {
            let Some(primary) = self.create_pool(config).await? else {
                return Ok(None);
            };
            let replica = config
                .replica_url
                .as_deref()
                .map(|url| {
                    build_pool(
                        url,
                        config.effective_replica_pool_size(),
                        config.connect_timeout_secs,
                    )
                })
                .transpose()?;

            Ok(Some(DatabaseTopology::from_pools(primary, replica)))
        }
    }
}

/// Default [`DatabasePoolProvider`] — the `deadpool + diesel-async` factory.
///
/// Delegates to the free function [`create_pool`]. This is the provider used
/// when no override is installed via
/// [`with_pool_provider`](crate::app::AppBuilder::with_pool_provider).
#[derive(Debug, Default, Clone, Copy)]
pub struct DieselDeadpoolPoolProvider;

impl DieselDeadpoolPoolProvider {
    /// Construct a new default provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DatabasePoolProvider for DieselDeadpoolPoolProvider {
    async fn create_pool(
        &self,
        config: &DatabaseConfig,
    ) -> Result<Option<Pool<AsyncPgConnection>>, PoolError> {
        create_pool(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::Duration;

    // ── after_commit tests ───────────────────────────────────────

    #[tokio::test]
    async fn register_after_commit_outside_tx_runs_eagerly() {
        // When called outside a db.tx block, the callback should run immediately.
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        register_after_commit(move || async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn register_after_commit_eager_failure_increments_failure_counter() {
        let before = AFTER_COMMIT_FAILURES_TOTAL.load(Ordering::Relaxed);

        register_after_commit(|| async {
            Err(crate::AutumnError::internal_server_error_msg(
                "deliberate eager after-commit failure",
            ))
        })
        .await;

        let after = AFTER_COMMIT_FAILURES_TOTAL.load(Ordering::Relaxed);
        assert!(
            after > before,
            "eager after_commit failures should be counted for recovery signals"
        );
    }

    #[tokio::test]
    async fn register_after_commit_inside_scope_defers_until_drained() {
        // Inside a task-local scope (simulating Db::tx), callbacks are deferred.
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();

        let registry = Arc::new(std::sync::Mutex::new(Vec::<CommitCallback>::new()));

        // Simulate being inside a db.tx by setting the task-local
        AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), async {
                register_after_commit(move || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await;
            })
            .await;

        // Callback must NOT have run yet
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        // Drain and run the callbacks (simulating post-commit)
        let callbacks: Vec<CommitCallback> = {
            let mut reg = registry.lock().unwrap();
            std::mem::take(&mut *reg)
        };
        for cb in callbacks {
            cb().await.unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn register_after_commit_on_rollback_callbacks_dropped() {
        // Callbacks registered inside a tx scope that is NOT drained are dropped.
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();

        let registry = Arc::new(std::sync::Mutex::new(Vec::<CommitCallback>::new()));

        AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), async {
                register_after_commit(move || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await;
            })
            .await;

        // Simulate rollback: drop the callbacks without running them
        drop(registry);

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn register_after_commit_callbacks_run_in_registration_order() {
        let order = Arc::new(std::sync::Mutex::new(Vec::<u32>::new()));
        let registry = Arc::new(std::sync::Mutex::new(Vec::<CommitCallback>::new()));

        let o1 = order.clone();
        let o2 = order.clone();
        let o3 = order.clone();

        AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), async {
                register_after_commit(move || async move {
                    o1.lock().unwrap().push(1);
                    Ok(())
                })
                .await;
                register_after_commit(move || async move {
                    o2.lock().unwrap().push(2);
                    Ok(())
                })
                .await;
                register_after_commit(move || async move {
                    o3.lock().unwrap().push(3);
                    Ok(())
                })
                .await;
            })
            .await;

        let callbacks: Vec<CommitCallback> = {
            let mut reg = registry.lock().unwrap();
            std::mem::take(&mut *reg)
        };
        for cb in callbacks {
            cb().await.unwrap();
        }

        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn production_after_commit_drain_preserves_registration_order() {
        let order = Arc::new(std::sync::Mutex::new(Vec::<u32>::new()));
        let (release_first, wait_first) = tokio::sync::oneshot::channel::<()>();

        let first_order = order.clone();
        let second_order = order.clone();
        let callbacks: Vec<CommitCallback> = vec![
            Box::new(move || {
                Box::pin(async move {
                    wait_first
                        .await
                        .expect("test should release first callback");
                    first_order.lock().unwrap().push(1);
                    Ok(())
                })
            }),
            Box::new(move || {
                Box::pin(async move {
                    second_order.lock().unwrap().push(2);
                    Ok(())
                })
            }),
        ];

        let drain = spawn_committed_after_commit_callbacks(callbacks)
            .expect("non-empty callback list should spawn a drain task");
        tokio::task::yield_now().await;

        assert_eq!(
            *order.lock().unwrap(),
            Vec::<u32>::new(),
            "later callbacks must wait for earlier callbacks to finish"
        );

        release_first
            .send(())
            .expect("first callback receiver alive");
        drain.await.expect("drain task should not panic");

        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
    }

    #[tokio::test]
    async fn production_after_commit_drain_isolates_panicking_callbacks() {
        let before = AFTER_COMMIT_FAILURES_TOTAL.load(Ordering::Relaxed);
        let ran_later = Arc::new(AtomicU64::new(0));
        let later = ran_later.clone();

        let callbacks: Vec<CommitCallback> = vec![
            Box::new(|| Box::pin(async { panic!("deliberate after_commit panic") })),
            Box::new(move || {
                Box::pin(async move {
                    later.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        ];

        let drain = spawn_committed_after_commit_callbacks(callbacks)
            .expect("non-empty callback list should spawn a drain task");
        drain.await.expect("panicking callback should be isolated");

        assert_eq!(
            ran_later.load(Ordering::SeqCst),
            1,
            "later callbacks must still run after an earlier callback panics"
        );
        let after = AFTER_COMMIT_FAILURES_TOTAL.load(Ordering::Relaxed);
        assert!(
            after > before,
            "panicking after_commit callbacks must increment the failure counter"
        );
    }

    #[tokio::test]
    async fn db_tx_rejects_ambient_after_commit_registry() {
        let registry = Arc::new(std::sync::Mutex::new(Vec::<CommitCallback>::new()));

        let err = AFTER_COMMIT_REGISTRY
            .scope(registry, async {
                reject_ambient_after_commit_registry_for_tx().expect_err(
                    "starting Db::tx inside an ambient transaction registry should fail",
                )
            })
            .await;

        assert!(
            err.to_string().contains("Nested Db::tx calls"),
            "unexpected nested transaction error: {err}"
        );
    }

    #[tokio::test]
    async fn register_after_commit_callback_error_is_swallowed() {
        // A failing callback is logged but doesn't panic or propagate.
        let registry = Arc::new(std::sync::Mutex::new(Vec::<CommitCallback>::new()));

        AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), async {
                register_after_commit(|| async {
                    Err(crate::AutumnError::internal_server_error_msg(
                        "deliberate error",
                    ))
                })
                .await;
            })
            .await;

        let callbacks: Vec<CommitCallback> = {
            let mut reg = registry.lock().unwrap();
            std::mem::take(&mut *reg)
        };
        // Running a failing callback should not panic
        for cb in callbacks {
            let _ = cb().await;
        }
    }

    // ── Pool provider trait tests ────────────────────────────────

    /// No-op provider for tests — always returns `Ok(None)` regardless of the
    /// supplied config. Verifies the trait actually overrides the default
    /// (which would otherwise build a pool from the URL).
    struct NoOpPoolProvider;

    impl DatabasePoolProvider for NoOpPoolProvider {
        async fn create_pool(
            &self,
            _config: &DatabaseConfig,
        ) -> Result<Option<Pool<AsyncPgConnection>>, PoolError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn pool_provider_trait_returns_supplied_pool() {
        // Even with a configured URL, the no-op provider returns None — proving
        // the trait can replace the default factory's behaviour.
        let config = DatabaseConfig {
            url: Some("postgres://localhost/ignored".to_owned()),
            ..Default::default()
        };
        let provider = NoOpPoolProvider;
        let pool = provider
            .create_pool(&config)
            .await
            .expect("no-op provider should succeed");
        assert!(
            pool.is_none(),
            "no-op provider must override default behaviour"
        );
    }

    #[tokio::test]
    async fn default_pool_provider_matches_free_function() {
        let config = DatabaseConfig::default();
        let via_provider = DieselDeadpoolPoolProvider::new()
            .create_pool(&config)
            .await
            .expect("default provider should succeed");
        let via_function = create_pool(&config).expect("free fn should succeed");
        assert_eq!(via_provider.is_none(), via_function.is_none());
    }

    // ── Pool creation tests ──────────────────────────────────────

    #[tokio::test]
    async fn default_pool_provider_respects_url_config() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            ..Default::default()
        };
        let provider = DieselDeadpoolPoolProvider::new();
        let pool = provider
            .create_pool(&config)
            .await
            .expect("default provider should succeed");
        assert!(
            pool.is_some(),
            "default provider should return Some when url is provided"
        );
    }

    #[test]
    fn create_pool_with_no_url_returns_none() {
        let config = DatabaseConfig::default();
        let pool = create_pool(&config).expect("should not fail with no URL");
        assert!(pool.is_none());
    }

    #[test]
    fn create_pool_with_url_returns_some() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            ..Default::default()
        };
        let pool = create_pool(&config).expect("should build pool from valid config");
        assert!(pool.is_some());
    }

    #[test]
    fn pool_respects_max_size() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            pool_size: 5,
            ..Default::default()
        };
        let pool = create_pool(&config)
            .expect("should build pool")
            .expect("should be Some");
        assert_eq!(pool.status().max_size, 5);
    }

    #[test]
    fn pool_clamps_size_to_one_if_zero() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            pool_size: 0,
            ..Default::default()
        };
        let pool = create_pool(&config)
            .expect("should build pool")
            .expect("should be Some");
        assert_eq!(
            pool.status().max_size,
            1,
            "Pool size should be clamped to 1"
        );
    }

    // ── Db extractor tests ───────────────────────────────────────

    #[test]
    fn database_topology_builds_primary_and_replica_pools() {
        let config = DatabaseConfig {
            primary_url: Some("postgres://localhost/primary".into()),
            replica_url: Some("postgres://localhost/replica".into()),
            primary_pool_size: Some(6),
            replica_pool_size: Some(2),
            ..Default::default()
        };

        let topology = create_topology(&config)
            .expect("topology should build")
            .expect("topology should be configured");

        assert_eq!(topology.primary().status().max_size, 6);
        assert_eq!(
            topology.replica().expect("replica pool").status().max_size,
            2
        );
        assert_eq!(topology.read().status().max_size, 2);
    }

    #[test]
    fn database_topology_single_url_builds_only_primary_pool() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/single".into()),
            pool_size: 5,
            ..Default::default()
        };

        let topology = create_topology(&config)
            .expect("topology should build")
            .expect("topology should be configured");

        assert_eq!(topology.primary().status().max_size, 5);
        assert!(topology.replica().is_none());
        assert_eq!(topology.read().status().max_size, 5);
    }

    #[test]
    fn config_runtime_drift_pool_applies_connect_timeout_to_wait_and_create() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            connect_timeout_secs: 7,
            ..Default::default()
        };
        let pool = create_pool(&config)
            .expect("should build pool")
            .expect("should be Some");

        let timeouts = pool.timeouts();
        assert_eq!(timeouts.wait, Some(Duration::from_secs(7)));
        assert_eq!(timeouts.create, Some(Duration::from_secs(7)));
    }

    #[derive(Clone)]
    struct TestDbState;

    impl DbState for TestDbState {
        fn pool(&self) -> Option<&Pool<AsyncPgConnection>> {
            None
        }
    }

    #[derive(Clone)]
    struct TestReadState {
        primary: Pool<AsyncPgConnection>,
    }

    impl DbState for TestReadState {
        fn pool(&self) -> Option<&Pool<AsyncPgConnection>> {
            Some(&self.primary)
        }
    }

    #[test]
    fn database_topology_read_pool_falls_back_to_primary() {
        let config = DatabaseConfig {
            url: Some("postgres://localhost/read-fallback".into()),
            pool_size: 3,
            ..Default::default()
        };
        let primary = create_pool(&config).unwrap().unwrap();
        let state = TestReadState { primary };

        assert_eq!(state.read_pool().expect("read pool").status().max_size, 3);
    }

    #[tokio::test]
    async fn db_extractor_rejects_when_no_pool() {
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use axum::routing::get;
        use tower::ServiceExt;

        async fn handler(_db: Db) -> &'static str {
            "ok"
        }

        let app = Router::new()
            .route("/", get(handler))
            .with_state(TestDbState);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn database_topology_primary_only_has_no_replica() {
        let config = DatabaseConfig {
            primary_url: Some("postgres://user:pass@localhost/db".to_string()),
            ..DatabaseConfig::default()
        };
        let topology = create_topology(&config).unwrap().unwrap();

        let primary = topology.primary().clone();

        let new_topology = DatabaseTopology::primary_only(primary);
        assert!(
            new_topology.replica().is_none(),
            "primary_only must set replica to None"
        );
    }

    #[tokio::test]
    async fn database_topology_from_pools_retains_replica() {
        let config = DatabaseConfig {
            primary_url: Some("postgres://user:pass@localhost/db".to_string()),
            replica_url: Some("postgres://user:pass@localhost/db_replica".to_string()),
            ..DatabaseConfig::default()
        };
        let topology = create_topology(&config).unwrap().unwrap();

        let primary = topology.primary().clone();
        let replica = topology.replica().cloned();

        let new_topology = DatabaseTopology::from_pools(primary, replica);
        assert!(
            new_topology.replica().is_some(),
            "from_pools must preserve the replica pool"
        );
    }
}
