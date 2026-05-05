//! Caching infrastructure for the Autumn framework.
//!
//! This module provides:
//!
//! - [`Cache`] — a trait abstracting over cache backends (moka by default,
//!   swap in Redis, memcached, etc.)
//! - [`MokaCache`] — the default, lock-free, in-process cache powered by
//!   [moka](https://docs.rs/moka) (behind the `cache-moka` feature)
//! - [`CacheResponseLayer`] — a Tower middleware that caches HTTP GET
//!   responses, usable via `#[intercept(CacheResponseLayer::new(...))]`
//! - [`CacheableResult`] — helper trait used by `#[cached(result)]` to
//!   only cache `Ok` values
//!
//! The `#[cached]` proc macro generates a per-function static `MokaCache`
//! for function-level memoization. The `CacheResponseLayer` operates at
//! the HTTP level using a shared `Arc<dyn Cache>`.
//!
//! # Swapping backends
//!
//! Implement the [`Cache`] trait for your backend:
//!
//! ```rust,ignore
//! use autumn_web::cache::Cache;
//!
//! #[derive(Clone)]
//! struct RedisCache { /* ... */ }
//!
//! impl Cache for RedisCache {
//!     fn get_value(&self, key: &str) -> Option<Box<dyn std::any::Any + Send + Sync>> { /* ... */ }
//!     fn insert_value(&self, key: &str, value: Box<dyn std::any::Any + Send + Sync>) { /* ... */ }
//!     fn invalidate(&self, key: &str) { /* ... */ }
//!     fn clear(&self) { /* ... */ }
//! }
//! ```

mod layer;
#[cfg(feature = "cache-moka")]
mod moka_impl;

pub use layer::{CacheResponseLayer, CacheResponseService};
#[cfg(feature = "cache-moka")]
pub use moka_impl::MokaCache;

use std::any::Any;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, RwLock};

// ── Global cache registry ────────────────────────────────────────────

/// Process-level shared cache backend.
///
/// Set once at startup by [`set_global_cache`]; read by every
/// `#[cached]`-annotated function to decide which store to use.
static GLOBAL_CACHE: RwLock<Option<Arc<dyn Cache>>> = RwLock::new(None);

/// Register (or replace) the process-level shared cache.
///
/// Called automatically by [`crate::app::AppBuilder`] when
/// `.with_cache_backend(...)` has been used. Also called by
/// [`crate::state::AppState::set_cache`] when a plugin installs a backend
/// during the startup-hook phase.
///
/// # Panics
///
/// Panics if the internal `RwLock` is poisoned.
pub fn set_global_cache(cache: Arc<dyn Cache>) {
    *GLOBAL_CACHE.write().expect("global cache lock poisoned") = Some(cache);
}

/// Return a clone of the process-level shared cache, if one is registered.
///
/// `None` means no global backend has been set and `#[cached]` functions
/// fall back to their per-function Moka stores.
///
/// # Panics
///
/// Panics if the internal `RwLock` is poisoned.
#[must_use]
pub fn global_cache() -> Option<Arc<dyn Cache>> {
    GLOBAL_CACHE
        .read()
        .expect("global cache lock poisoned")
        .clone()
}

/// Remove the process-level shared cache.
///
/// Primarily useful in tests that need per-test isolation.
///
/// # Panics
///
/// Panics if the internal `RwLock` is poisoned.
pub fn clear_global_cache() {
    *GLOBAL_CACHE.write().expect("global cache lock poisoned") = None;
}

// ── Cache trait ──────────────────────────────────────────────────────

/// Raw JSON bytes stored by serializing cache backends (e.g. Redis).
///
/// Backends that cannot store `Arc<dyn Any>` directly (because values must
/// survive across process boundaries) return this from [`Cache::get_value`]
/// instead. [`get_cached`] and [`insert_cached`] transparently deserialize it
/// back into the concrete type `V` using `serde_json`.
#[derive(Clone)]
pub struct RawCacheBytes(pub Vec<u8>);

/// A type-erased, thread-safe cache store.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// handlers and tasks. Values are stored as `Arc<dyn Any>` for type
/// erasure, allowing a single cache instance to store heterogeneous
/// types from different `#[cached]` functions.
///
/// Use the free functions [`get`] / [`insert`] for non-serde types (e.g.
/// HTTP responses in [`CacheResponseLayer`]), or [`get_cached`] /
/// [`insert_cached`] for types that also implement `serde` — which is
/// required for cross-replica backends like Redis.
pub trait Cache: Send + Sync + 'static {
    /// Retrieve a type-erased value by key. Returns `None` on miss.
    ///
    /// Backends that store serialized data (e.g. Redis) may return
    /// <code>Arc<[RawCacheBytes]></code> here; [`get_cached`] handles the
    /// JSON deserialization transparently.
    fn get_value(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;

    /// Store a type-erased value by key.
    fn insert_value(&self, key: &str, value: Arc<dyn Any + Send + Sync>);

    /// Remove a specific key.
    fn invalidate(&self, key: &str);

    /// Remove all entries.
    fn clear(&self);

    /// Store pre-serialized JSON bytes for backends that persist data across
    /// process boundaries (e.g. Redis). The default is a no-op; in-process
    /// backends store values via [`insert_value`] instead.
    ///
    /// `ttl` carries the same time-to-live that was declared on the
    /// `#[cached(ttl = "…")]` attribute so backends can apply native expiry
    /// (e.g. Redis `SET EX`). `None` means no expiry.
    ///
    /// [`insert_value`]: Cache::insert_value
    fn insert_raw_bytes(&self, _key: &str, _bytes: Vec<u8>, _ttl: Option<std::time::Duration>) {}
}

// ── Typed convenience functions ──────────────────────────────────────

/// Typed get: retrieve and downcast a cached value.
///
/// Returns `None` if the key is absent or the stored type doesn't
/// match `V`. Works with any `Cache` implementation.
///
/// For cross-replica backends (Redis) use [`get_cached`] instead, which
/// also handles JSON deserialization of [`RawCacheBytes`].
pub fn get<V: Clone + Send + Sync + 'static>(cache: &dyn Cache, key: &str) -> Option<V> {
    cache
        .get_value(key)
        .and_then(|arc| arc.downcast_ref::<V>().cloned())
}

/// Typed insert: wrap the value in an `Arc` and store it.
///
/// Works with any `Cache` implementation.
///
/// For cross-replica backends (Redis) use [`insert_cached`] instead,
/// which also serializes the value for storage across process boundaries.
pub fn insert<V: Clone + Send + Sync + 'static>(cache: &dyn Cache, key: &str, value: V) {
    cache.insert_value(key, Arc::new(value));
}

/// Serde-aware get: retrieve a cached value, deserializing from JSON if needed.
///
/// First tries a direct in-memory downcast (fast path for `MokaCache`). If
/// that fails — because the backend stored [`RawCacheBytes`] (e.g. Redis) —
/// the bytes are deserialized with `serde_json`. This is what the `#[cached]`
/// macro uses so that values survive across replicas when a shared backend
/// is configured.
pub fn get_cached<V>(cache: &dyn Cache, key: &str) -> Option<V>
where
    V: Clone + serde::de::DeserializeOwned + Send + Sync + 'static,
{
    let arc = cache.get_value(key)?;
    // Fast path: in-memory backend stored the concrete type directly.
    if let Some(v) = arc.downcast_ref::<V>() {
        return Some(v.clone());
    }
    // Slow path: serializing backend (e.g. Redis) stored RawCacheBytes.
    arc.downcast_ref::<RawCacheBytes>()
        .and_then(|raw| serde_json::from_slice::<V>(&raw.0).ok())
}

/// Serde-aware insert: store the value both in-memory and as JSON bytes.
///
/// Calls [`Cache::insert_value`] (for in-process backends like Moka) **and**
/// [`Cache::insert_raw_bytes`] (for cross-replica backends like Redis). This
/// is what the `#[cached]` macro uses so that the stored value is accessible
/// both within the same process and on other replicas.
///
/// `ttl` is forwarded verbatim to [`Cache::insert_raw_bytes`] so backends
/// like Redis can apply a native entry expiry (e.g. `SET EX`). In-process
/// backends (Moka) manage TTL via the per-function static cache instance
/// and ignore this parameter.
pub fn insert_cached<V>(cache: &dyn Cache, key: &str, value: V, ttl: Option<std::time::Duration>)
where
    V: Clone + serde::Serialize + Send + Sync + 'static,
{
    // In-memory path (MokaCache, CountingCache in tests, …)
    cache.insert_value(key, Arc::new(value.clone()));
    // Serialized path (RedisCache, any cross-replica backend)
    if let Ok(bytes) = serde_json::to_vec(&value) {
        cache.insert_raw_bytes(key, bytes, ttl);
    }
}

// ── CacheableResult trait ────────────────────────────────────────────

/// Helper trait used by `#[cached(result)]` to extract the `Ok` type
/// from a `Result<T, E>` return type at the type level.
///
/// This avoids the need for the proc macro to syntactically parse
/// generic arguments out of the return type.
pub trait CacheableResult {
    /// The success type to cache.
    type Ok: Clone;
    /// The error type (passed through uncached).
    type Err;

    /// Convert into a standard `Result` for pattern matching.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the original result was an error.
    fn into_result(self) -> Result<Self::Ok, Self::Err>;
    /// Wrap a cached `Ok` value back into the original result type.
    fn from_ok(ok: Self::Ok) -> Self;
}

impl<T: Clone, E> CacheableResult for Result<T, E> {
    type Ok = T;
    type Err = E;

    fn into_result(self) -> Self {
        self
    }

    fn from_ok(ok: T) -> Self {
        Ok(ok)
    }
}

// ── Cache key helper ─────────────────────────────────────────────────

/// Build a cache key from a function name and its hashable arguments.
///
/// Used by `#[cached]` macro-generated code. The key is
/// `"{fn_name}:{hash_hex}"` where the hash is a 64-bit `DefaultHasher`
/// digest of the argument tuple.
#[must_use]
pub fn make_cache_key<K: Hash>(fn_name: &str, args: &K) -> String {
    let mut hasher = DefaultHasher::new();
    args.hash(&mut hasher);
    format!("{}:{:x}", fn_name, hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_deterministic() {
        let k1 = make_cache_key("get_user", &(42_i64,));
        let k2 = make_cache_key("get_user", &(42_i64,));
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_fn_name() {
        let k1 = make_cache_key("get_user", &(42_i64,));
        let k2 = make_cache_key("find_user", &(42_i64,));
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_args() {
        let k1 = make_cache_key("get_user", &(1_i64,));
        let k2 = make_cache_key("get_user", &(2_i64,));
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_no_args() {
        let k = make_cache_key("get_config", &());
        assert!(k.starts_with("get_config:"));
    }
}
