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
use std::hash::{Hash, Hasher};

/// A fast, non-cryptographic hash function (FNV-1a) optimized for small keys.
///
/// ⚡ Bolt Optimization: Replacing the default `SipHash` (`DefaultHasher`) with FNV-1a
/// removes cryptographic overhead, significantly improving cache key generation speed
/// for short strings and tuple arguments.
pub struct FnvHasher(u64);

impl Default for FnvHasher {
    #[inline]
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut hash = self.0;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = hash;
    }
}
use std::sync::Arc;

// ── Cache trait ──────────────────────────────────────────────────────

/// A type-erased, thread-safe cache store.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// handlers and tasks. Values are stored as `Arc<dyn Any>` for type
/// erasure, allowing a single cache instance to store heterogeneous
/// types from different `#[cached]` functions.
///
/// Use the free functions [`get`] and [`insert`] for typed access
/// that handles `Arc` wrapping and downcasting automatically.
pub trait Cache: Send + Sync + 'static {
    /// Retrieve a type-erased value by key. Returns `None` on miss.
    fn get_value(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;

    /// Store a type-erased value by key.
    fn insert_value(&self, key: &str, value: Arc<dyn Any + Send + Sync>);

    /// Remove a specific key.
    fn invalidate(&self, key: &str);

    /// Remove all entries.
    fn clear(&self);
}

// ── Typed convenience functions ──────────────────────────────────────

/// Typed get: retrieve and downcast a cached value.
///
/// Returns `None` if the key is absent or the stored type doesn't
/// match `V`. Works with any `Cache` implementation.
pub fn get<V: Clone + Send + Sync + 'static>(cache: &dyn Cache, key: &str) -> Option<V> {
    cache
        .get_value(key)
        .and_then(|arc| arc.downcast_ref::<V>().cloned())
}

/// Typed insert: wrap the value in an `Arc` and store it.
///
/// Works with any `Cache` implementation.
pub fn insert<V: Clone + Send + Sync + 'static>(cache: &dyn Cache, key: &str, value: V) {
    cache.insert_value(key, Arc::new(value));
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
/// `"{fn_name}:{hash_hex}"` where the hash is a 64-bit `FnvHasher`
/// digest of the argument tuple.
#[must_use]
pub fn make_cache_key<K: Hash>(fn_name: &str, args: &K) -> String {
    let mut hasher = FnvHasher::default();
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
