//! In-memory function result cache for the `#[cached]` macro.
//!
//! Provides [`CacheStore`], a thread-safe, TTL-aware cache with optional
//! max-entry eviction. Each `#[cached]` function gets its own static
//! `CacheStore` instance, initialized on first call.
//!
//! # Design
//!
//! - **Thread-safe**: uses `std::sync::Mutex` (non-async — lock is held
//!   only during fast HashMap operations, never across `.await` points).
//! - **TTL**: optional per-entry expiration checked on read.
//! - **Max entries**: when set, the oldest entry is evicted on insert
//!   (FIFO order via `IndexMap`).
//! - **No external dependencies**: uses only `std` + `indexmap` (already
//!   in the workspace).

use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use indexmap::IndexMap;

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
    fn into_result(self) -> Result<Self::Ok, Self::Err>;
    /// Wrap a cached `Ok` value back into the original result type.
    fn from_ok(ok: Self::Ok) -> Self;
}

impl<T: Clone, E> CacheableResult for Result<T, E> {
    type Ok = T;
    type Err = E;

    fn into_result(self) -> Result<T, E> {
        self
    }

    fn from_ok(ok: T) -> Self {
        Ok(ok)
    }
}

struct CacheEntry<V> {
    value: V,
    inserted_at: Instant,
}

/// A thread-safe, TTL-aware cache store.
///
/// Created by the `#[cached]` macro — not intended for direct construction.
pub struct CacheStore<K, V> {
    entries: Mutex<IndexMap<K, CacheEntry<V>>>,
    ttl: Option<Duration>,
    max: Option<usize>,
}

impl<K, V> CacheStore<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Create a new cache store with optional TTL and max-entry limits.
    #[must_use]
    pub fn new(ttl: Option<Duration>, max: Option<usize>) -> Self {
        Self {
            entries: Mutex::new(IndexMap::new()),
            ttl,
            max,
        }
    }

    /// Look up a cached value by key.
    ///
    /// Returns `None` if the key is absent or the entry has expired.
    pub fn get(&self, key: &K) -> Option<V> {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.get(key)?;

        if let Some(ttl) = self.ttl {
            if entry.inserted_at.elapsed() > ttl {
                map.shift_remove(key);
                return None;
            }
        }

        Some(entry.value.clone())
    }

    /// Insert a value into the cache.
    ///
    /// If the cache is at max capacity, the oldest entry is evicted first.
    pub fn insert(&self, key: K, value: V) {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        // Evict expired entries first
        if self.ttl.is_some() {
            let ttl = self.ttl.unwrap();
            map.retain(|_, entry| entry.inserted_at.elapsed() <= ttl);
        }

        // Evict oldest if at capacity
        if let Some(max) = self.max {
            while map.len() >= max {
                map.shift_remove_index(0);
            }
        }

        map.insert(
            key,
            CacheEntry {
                value,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Remove all entries from the cache.
    pub fn clear(&self) {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();
    }

    /// Return the number of entries currently in the cache (including expired).
    pub fn len(&self) -> usize {
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.len()
    }

    /// Returns `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn basic_insert_and_get() {
        let cache: CacheStore<String, i32> = CacheStore::new(None, None);
        cache.insert("a".into(), 1);
        assert_eq!(cache.get(&"a".into()), Some(1));
        assert_eq!(cache.get(&"b".into()), None);
    }

    #[test]
    fn ttl_expiry() {
        let cache: CacheStore<String, i32> =
            CacheStore::new(Some(Duration::from_millis(50)), None);
        cache.insert("a".into(), 1);
        assert_eq!(cache.get(&"a".into()), Some(1));
        thread::sleep(Duration::from_millis(80));
        assert_eq!(cache.get(&"a".into()), None);
    }

    #[test]
    fn max_entries_evicts_oldest() {
        let cache: CacheStore<String, i32> = CacheStore::new(None, Some(2));
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        cache.insert("c".into(), 3);
        assert_eq!(cache.get(&"a".into()), None); // evicted
        assert_eq!(cache.get(&"b".into()), Some(2));
        assert_eq!(cache.get(&"c".into()), Some(3));
    }

    #[test]
    fn clear_removes_all() {
        let cache: CacheStore<String, i32> = CacheStore::new(None, None);
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn overwrite_existing_key() {
        let cache: CacheStore<String, i32> = CacheStore::new(None, None);
        cache.insert("a".into(), 1);
        cache.insert("a".into(), 2);
        assert_eq!(cache.get(&"a".into()), Some(2));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;

        let cache = Arc::new(CacheStore::<i32, i32>::new(None, None));
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let c = Arc::clone(&cache);
                thread::spawn(move || {
                    c.insert(i, i * 10);
                    c.get(&i)
                })
            })
            .collect();

        for (i, h) in handles.into_iter().enumerate() {
            let val = h.join().unwrap();
            assert_eq!(val, Some((i as i32) * 10));
        }
    }
}
