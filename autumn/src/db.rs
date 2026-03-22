//! Database connection pool management.
//!
//! Uses `diesel-async` with `deadpool` for async Postgres connections.
//! The pool is created at startup and stored in [`AppState`](crate::AppState).
//!
//! When no `database.url` is configured, [`create_pool`] returns `Ok(None)`
//! and the application runs without a database — useful for static-site or
//! API-gateway use cases.

use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;

use crate::config::DatabaseConfig;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;

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
}
