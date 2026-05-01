//! Database connection pool and extractor.
//!
//! This module provides async Postgres connectivity via `diesel-async` with
//! the `deadpool` connection pool. The pool is created at startup by
//! [`AppBuilder::run`](crate::app::AppBuilder::run) and stored in
//! [`crate::state::AppState`].
//!
//! When no `database.url` is configured, [`create_pool`] returns `Ok(None)`
//! and the application runs without a database -- useful for static-site or
//! API-gateway use cases.
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
use std::time::Duration;
use tracing::Instrument as _;

use crate::config::DatabaseConfig;
use crate::error::AutumnError;

/// Trait to abstract the state requirement for the `Db` extractor.
/// This breaks the circular dependency between the database extractor
/// and the central `AppState`.
pub trait DbState {
    /// Returns the database connection pool, if configured.
    fn pool(&self) -> Option<&Pool<AsyncPgConnection>>;
}

/// Error type for pool creation failures.
///
/// Alias for the deadpool `BuildError`. Returned by [`create_pool`] when
/// the pool cannot be constructed (e.g., invalid max-size configuration).
pub type PoolError = diesel_async::pooled_connection::deadpool::BuildError;

/// Create a connection pool from the database configuration.
///
/// Returns `Ok(None)` if no database URL is configured (`database.url` is
/// absent or `null` in `autumn.toml`).
///
/// # Errors
///
/// Returns [`PoolError`] if the pool cannot be built (e.g., invalid
/// max-size configuration).
pub fn create_pool(config: &DatabaseConfig) -> Result<Option<Pool<AsyncPgConnection>>, PoolError> {
    let Some(url) = &config.url else {
        return Ok(None);
    };

    let timeout = Duration::from_secs(config.connect_timeout_secs);
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
    let pool = Pool::builder(manager)
        .max_size(config.pool_size.max(1))
        .wait_timeout(Some(timeout))
        .create_timeout(Some(timeout))
        .runtime(deadpool::Runtime::Tokio1)
        .build()?;

    Ok(Some(pool))
}

// ── Db extractor ─────────────────────────────────────────────

/// Connection type managed by the deadpool pool.
type PooledConnection = diesel_async::pooled_connection::deadpool::Object<AsyncPgConnection>;

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
/// If no database is configured (i.e., `database.url` is absent),
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
}

impl std::ops::Deref for Db {
    type Target = AsyncPgConnection;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl std::ops::DerefMut for Db {
    fn deref_mut(&mut self) -> &mut Self::Target {
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

        Ok(Self { conn, span })
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
    use std::time::Duration;

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

    #[tokio::test]
    async fn db_extractor_rejects_when_no_pool() {
        use crate::state::AppState;
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use axum::routing::get;
        use tower::ServiceExt;

        async fn handler(_db: Db) -> &'static str {
            "ok"
        }

        let app = Router::new().route("/", get(handler)).with_state(AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
        });

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
