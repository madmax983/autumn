use std::time::Duration;
use autumn_web::idempotency::{MemoryIdempotencyStore, IdempotencyStore, IdempotencyRecord};

#[test]
fn havoc_idempotency_store_survives_max_duration() {
    let store = MemoryIdempotencyStore::new(Duration::from_secs(60));

    // Panic 1: try_lock with Duration::MAX
    let lock_success = store.try_lock("test_key", Duration::MAX);
    assert!(lock_success);

    // Panic 2: set with Duration::MAX
    store.set("test_key2", IdempotencyRecord {
        status: 200,
        headers: vec![],
        body: vec![],
        metadata: vec![],
    }, vec![], Duration::MAX);
}
