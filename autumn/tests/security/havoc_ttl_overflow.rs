use autumn_web::idempotency::{IdempotencyRecord, IdempotencyStore, MemoryIdempotencyStore};
use std::time::Duration;

#[test]
fn test_idempotency_ttl_overflow() {
    let store = MemoryIdempotencyStore::new(Duration::from_secs(60));
    let ttl = Duration::MAX;

    // Test `try_lock_owned`
    let locked = store.try_lock_owned("key", "owner-a", ttl);
    assert!(
        locked,
        "Lock should be acquired successfully, failing closed the TTL instead of panicking"
    );

    // Test `set`
    let record = IdempotencyRecord {
        status: 200,
        headers: Default::default(),
        body: vec![],
        metadata: vec![],
    };
    store.set("key", record, vec![], ttl);
}
