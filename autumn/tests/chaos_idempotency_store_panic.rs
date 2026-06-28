use autumn_web::idempotency::{IdempotencyRecord, IdempotencyStore, MemoryIdempotencyStore};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn idempotency_store_ttl_panic() {
    let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));
    let key = "test_key";
    let record = IdempotencyRecord {
        status: 200,
        headers: Default::default(),
        body: vec![],
        metadata: vec![],
    };

    store.set(key, record.clone(), vec![], Duration::MAX);
}

#[test]
fn idempotency_store_lock_ttl_panic() {
    let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));
    let key = "test_key";

    store.try_lock(key, Duration::MAX);
}
