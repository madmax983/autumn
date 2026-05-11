//! Integration test: two replicas sharing a Redis backend see the same cached
//! data and invalidations within one round-trip.
//!
//! Mirrors the production topology: two `bookmarks-distributed` replicas
//! share a single Redis instance (see `docker-compose.yml`). Any write to the
//! cache on replica A must be readable by replica B, and any invalidation from
//! replica A must immediately remove the entry on replica B.

use autumn_cache_redis::RedisCache;
use autumn_web::cache::{Cache as _, get, insert};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis as RedisImage;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker (testcontainers)"]
async fn cross_replica_cached_count_consistency() {
    let container = RedisImage::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");

    // Two "replicas" that share the same Redis namespace, just like the two
    // bookmarks-distributed replicas behind the nginx load-balancer.
    let replica_a = RedisCache::connect(&url, "bookmarks:cache").await.unwrap();
    let replica_b = RedisCache::connect(&url, "bookmarks:cache").await.unwrap();

    // Replica A caches a bookmark count (the type used by cached_bookmark_count).
    let count_key = autumn_web::cache::make_cache_key("cached_bookmark_count", &());
    insert(&replica_a, &count_key, 7_i64);

    // Replica B immediately sees the cached value — no TTL wait, no gossip lag.
    let seen: Option<i64> = get(&replica_b, &count_key);
    assert_eq!(
        seen,
        Some(7),
        "replica B must read the count written by replica A"
    );

    // Replica A invalidates after a write (e.g. a new bookmark was created).
    replica_a.invalidate(&count_key);

    // Replica B must observe the invalidation within the same round-trip.
    let after_invalidate: Option<i64> = get(&replica_b, &count_key);
    assert!(
        after_invalidate.is_none(),
        "replica B must see replica A's invalidation immediately"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker (testcontainers)"]
async fn cross_replica_invalidation_under_50ms() {
    let container = RedisImage::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");

    let replica_a = RedisCache::connect(&url, "bookmarks:cache").await.unwrap();
    let replica_b = RedisCache::connect(&url, "bookmarks:cache").await.unwrap();

    let key = autumn_web::cache::make_cache_key("cached_bookmark_count", &());
    let start = std::time::Instant::now();

    insert(&replica_a, &key, 42_i64);
    let _seen: Option<i64> = get(&replica_b, &key);
    replica_a.invalidate(&key);
    let _gone: Option<i64> = get(&replica_b, &key);

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 50,
        "full write+read+invalidate+read cycle took {elapsed:?}, must be < 50 ms"
    );
}
