#![cfg(feature = "ws")]
use autumn_web::idempotency::{IdempotencyStore, MemoryIdempotencyStore};
use loom::thread;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn idempotency_store_ttl_panic() {
    loom::model(|| {
        let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));
        let key = "test_key";

        let s1 = store.clone();
        let t1 = thread::spawn(move || {
            // Using a very large duration can cause an overflow panic when added to Instant::now()
            s1.try_lock(key, Duration::MAX);
        });

        t1.join().unwrap();
    });
}
