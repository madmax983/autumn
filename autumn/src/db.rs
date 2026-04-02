//! Database connection pool and extractor.
//!
//! This module provides async Postgres connectivity via `diesel-async` with
//! the `deadpool` connection pool. The pool is created at startup by
//! [`AppBuilder::run`](crate::app::AppBuilder::run) and stored in
//! [`AppState`].
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

use crate::config::DatabaseConfig;
use crate::error::AutumnError;
use crate::state::AppState;

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

    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
    let pool = Pool::builder(manager).max_size(config.pool_size).build()?;

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
pub struct Db(PooledConnection);

impl std::ops::Deref for Db {
    type Target = AsyncPgConnection;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Db {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl FromRequestParts<AppState> for Db {
    type Rejection = AutumnError;

    async fn from_request_parts(
        _parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let pool = state
            .pool
            .as_ref()
            .ok_or_else(|| AutumnError::service_unavailable_msg("Database not configured"))?;

        let conn = pool.get().await.map_err(|e| {
            tracing::error!("Failed to acquire database connection: {e}");
            AutumnError::service_unavailable_msg(e.to_string())
        })?;

        Ok(Self(conn))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;

    // ── Pool creation tests ──────────────────────────────────────

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

    // ── Db extractor tests ───────────────────────────────────────

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

        let app = Router::new().route("/", get(handler)).with_state(AppState {
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        });

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
