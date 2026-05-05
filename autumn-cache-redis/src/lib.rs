//! Redis-backed shared cache for Autumn applications.
//!
//! This crate provides [`RedisCache`], an implementation of the
//! `autumn_web::cache::Cache` trait backed by Redis, plus
//! [`RedisCachePlugin`] which wires it into the app via the plugin system.
//!
//! # Usage
//!
//! ```toml
//! # autumn.toml
//! [cache]
//! backend = "redis"
//!
//! [cache.redis]
//! url = "redis://redis:6379"
//! key_prefix = "myapp:cache"
//! ```
//!
//! ```rust,ignore
//! use autumn_cache_redis::RedisCachePlugin;
//!
//! autumn_web::app()
//!     .plugin(RedisCachePlugin::new())
//!     .routes(routes![...])
//!     .run()
//!     .await;
//! ```
//!
//! Values are serialized as JSON so they survive across replicas and restarts.
//! Invalidation on replica A is immediately visible on replica B — no TTL lag.

use std::any::Any;
use std::sync::Arc;

use autumn_web::cache::Cache;
use redis::aio::ConnectionManager;
use redis::AsyncCommands as _;
use serde_json::Value as JsonValue;
use thiserror::Error;
use tracing::{debug, warn};

/// Errors that can occur when constructing or using a [`RedisCache`].
#[derive(Debug, Error)]
pub enum RedisCacheError {
    #[error("Redis connection error: {0}")]
    Connection(#[from] redis::RedisError),
    #[error("missing Redis URL in cache config")]
    MissingUrl,
}

/// A [`Cache`] implementation backed by Redis.
///
/// Values are stored as JSON blobs with an optional TTL. Multiple replicas
/// share the same Redis namespace so invalidations propagate instantly.
#[derive(Clone)]
pub struct RedisCache {
    manager: ConnectionManager,
    key_prefix: String,
}

impl RedisCache {
    /// Connect using an explicit URL and key prefix.
    ///
    /// # Errors
    ///
    /// Returns [`RedisCacheError::Connection`] if the initial connection fails.
    pub async fn connect(url: &str, key_prefix: impl Into<String>) -> Result<Self, RedisCacheError> {
        let client = redis::Client::open(url)?;
        let manager = ConnectionManager::new(client).await?;
        Ok(Self {
            manager,
            key_prefix: key_prefix.into(),
        })
    }

    /// Build a `RedisCache` from the `[cache]` section of `autumn.toml`.
    ///
    /// # Errors
    ///
    /// Returns [`RedisCacheError::MissingUrl`] if `cache.redis.url` is absent.
    /// Returns [`RedisCacheError::Connection`] on connection failure.
    pub async fn from_config(
        config: &autumn_web::config::CacheRedisConfig,
    ) -> Result<Self, RedisCacheError> {
        let url = config.url.as_deref().ok_or(RedisCacheError::MissingUrl)?;
        Self::connect(url, &config.key_prefix).await
    }

    fn prefixed(&self, key: &str) -> String {
        format!("{}:{}", self.key_prefix, key)
    }
}

/// Wraps a JSON-serializable value so the `Arc<dyn Any>` can be
/// restored from the raw bytes we get out of Redis.
#[derive(Clone)]
struct JsonBytes(Vec<u8>);

impl Cache for RedisCache {
    fn get_value(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        let prefixed = self.prefixed(key);
        let mut conn = self.manager.clone();
        // Run a blocking get on the Tokio current-thread context.
        // `Cache` is synchronous per the trait contract; callers that need
        // truly async access should use `RedisCache` directly.
        let result: Option<Vec<u8>> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { conn.get(&prefixed).await.ok().flatten() })
        });
        result.map(|bytes| Arc::new(JsonBytes(bytes)) as Arc<dyn Any + Send + Sync>)
    }

    fn insert_value(&self, key: &str, value: Arc<dyn Any + Send + Sync>) {
        let prefixed = self.prefixed(key);
        let mut conn = self.manager.clone();

        // Attempt to serialize the value as JSON. We check a few common types.
        let json_bytes: Option<Vec<u8>> = if let Some(json) = value.downcast_ref::<JsonBytes>() {
            Some(json.0.clone())
        } else if let Some(s) = value.downcast_ref::<String>() {
            serde_json::to_vec(&JsonValue::String(s.clone())).ok()
        } else if let Some(n) = value.downcast_ref::<i64>() {
            serde_json::to_vec(&JsonValue::Number((*n).into())).ok()
        } else if let Some(n) = value.downcast_ref::<i32>() {
            serde_json::to_vec(&JsonValue::Number((*n).into())).ok()
        } else {
            warn!(key, "RedisCache: cannot serialize value type, skipping insert");
            None
        };

        if let Some(bytes) = json_bytes {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    let _: Result<(), _> = conn.set(&prefixed, bytes).await;
                });
            });
            debug!(key, "RedisCache: inserted");
        }
    }

    fn invalidate(&self, key: &str) {
        let prefixed = self.prefixed(key);
        let mut conn = self.manager.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let _: Result<(), _> = conn.del(&prefixed).await;
            });
        });
        debug!(key, "RedisCache: invalidated");
    }

    fn clear(&self) {
        // Scan-and-delete all keys under our prefix.
        let pattern = format!("{}:*", self.key_prefix);
        let mut conn = self.manager.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let keys: Vec<String> = redis::cmd("KEYS")
                    .arg(&pattern)
                    .query_async(&mut conn)
                    .await
                    .unwrap_or_default();
                if !keys.is_empty() {
                    let _: Result<(), _> = conn.del(keys).await;
                }
            });
        });
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────────

/// Autumn plugin that wires `RedisCache` as the global application cache.
///
/// Reads `[cache.redis]` from the active `AutumnConfig` and installs a
/// `RedisCache` into `AppState` via `with_cache_backend`.
///
/// When `cache.backend != "redis"` the plugin is a no-op and the default
/// per-function Moka caches continue to work.
pub struct RedisCachePlugin;

impl RedisCachePlugin {
    /// Create the plugin.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for RedisCachePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl autumn_web::plugin::Plugin for RedisCachePlugin {
    fn build(self, app: autumn_web::app::AppBuilder) -> autumn_web::app::AppBuilder {
        app.on_startup(|state| async move {
            // Read the config the framework already stored as an extension.
            let config = state
                .extension::<autumn_web::config::AutumnConfig>()
                .expect("AutumnConfig must be registered before RedisCachePlugin startup");

            if !config.cache.is_redis() {
                return Ok(());
            }

            let redis_cfg = &config.cache.redis;
            let cache = RedisCache::from_config(redis_cfg).await.map_err(|e| {
                autumn_web::AutumnError::service_unavailable_msg(format!(
                    "Failed to connect RedisCache: {e}"
                ))
            })?;

            state.set_cache(Arc::new(cache));
            tracing::info!("RedisCache registered as global application cache");
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::redis::Redis as RedisImage;

    #[tokio::test(flavor = "multi_thread")]
    async fn redis_cache_insert_get_invalidate() {
        let container = RedisImage::default().start().await.unwrap();
        let port = container.get_host_port_ipv4(6379).await.unwrap();
        let url = format!("redis://127.0.0.1:{port}");

        let cache = RedisCache::connect(&url, "test").await.unwrap();

        // Insert a string
        autumn_web::cache::insert(&cache, "hello", "world".to_string());

        // get_value returns JsonBytes wrapping the JSON
        let raw = cache.get_value("hello").expect("should be present");
        // Downcast to JsonBytes and parse
        let bytes = raw.downcast_ref::<JsonBytes>().expect("JsonBytes");
        let v: serde_json::Value = serde_json::from_slice(&bytes.0).unwrap();
        assert_eq!(v, serde_json::json!("world"));

        // Invalidate
        cache.invalidate("hello");
        assert!(cache.get_value("hello").is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn redis_cache_cross_replica_invalidation() {
        let container = RedisImage::default().start().await.unwrap();
        let port = container.get_host_port_ipv4(6379).await.unwrap();
        let url = format!("redis://127.0.0.1:{port}");

        // Two "replicas" sharing the same Redis
        let replica_a = RedisCache::connect(&url, "xreplica").await.unwrap();
        let replica_b = RedisCache::connect(&url, "xreplica").await.unwrap();

        // A writes
        let start = std::time::Instant::now();
        autumn_web::cache::insert(&replica_a, "key", "value".to_string());

        // B can read it
        let raw = replica_b.get_value("key").expect("replica B should see the key");
        let elapsed = start.elapsed();

        let bytes = raw.downcast_ref::<JsonBytes>().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes.0).unwrap();
        assert_eq!(v, serde_json::json!("value"));

        // A invalidates
        replica_a.invalidate("key");

        // B no longer sees it (within one round-trip = < 50 ms p99)
        assert!(replica_b.get_value("key").is_none());
        // Verify we're well within the < 50 ms SLA from the issue
        assert!(elapsed.as_millis() < 50, "cross-replica lag {elapsed:?} exceeded 50 ms");
    }
}
