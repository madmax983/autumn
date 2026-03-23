//! Database connection pool and extractor.
//!
//! Uses `diesel-async` with `deadpool` for async Postgres connections.
//! The pool is created at startup and stored in [`AppState`](crate::AppState).
//!
//! When no `database.url` is configured, [`create_pool`] returns `Ok(None)`
//! and the application runs without a database — useful for static-site or
//! API-gateway use cases.
//!
//! The [`Db`] extractor acquires a connection from the pool for each request:
//!
//! ```ignore
//! #[get("/users")]
//! async fn list(db: Db) -> AutumnResult<Json<Vec<User>>> {
//!     let users = users::table.load(&mut *db).await?;
//!     Ok(Json(users))
//! }
//! ```

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;

use crate::AppState;
use crate::config::DatabaseConfig;
use crate::error::AutumnError;

/// Error type for pool creation failures.
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
/// Declare `db: Db` in your handler signature to get a pooled
/// connection to Postgres. The connection is returned to the pool
/// when `Db` is dropped.
///
/// ```ignore
/// #[get("/users")]
/// async fn list(db: Db) -> AutumnResult<Json<Vec<User>>> {
///     let users = users::table.load(&mut *db).await?;
///     Ok(Json(users))
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
        let pool = state.pool.as_ref().ok_or_else(|| {
            AutumnError::bad_request(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "Database not configured",
            ))
            .with_status(StatusCode::SERVICE_UNAVAILABLE)
        })?;

        let conn = pool.get().await.map_err(|e| {
            eprintln!("Failed to acquire database connection: {e}");
            AutumnError::bad_request(std::io::Error::other(e.to_string()))
                .with_status(StatusCode::SERVICE_UNAVAILABLE)
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
        use axum::http::Request;
        use axum::routing::get;
        use tower::ServiceExt;

        async fn handler(_db: Db) -> &'static str {
            "ok"
        }

        let app = Router::new()
            .route("/", get(handler))
            .with_state(AppState { pool: None });

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
