use autumn_web::idempotency::{IdempotencyRecord, IdempotencyStore, MemoryIdempotencyStore};
use std::time::Duration;

#[test]
fn havoc_idempotency_ttl_overflow() {
    let store = MemoryIdempotencyStore::new(Duration::from_secs(60));
    let record = IdempotencyRecord {
        status: 200,
        headers: vec![],
        body: vec![],
        metadata: Default::default(),
    };

    // This will panic if direct addition Instant::now() + ttl is used
    store.set("havoc_key", record, vec![], Duration::MAX);

    // This will also panic if direct addition is used for locks
    store.try_lock("havoc_lock", Duration::MAX);
}
