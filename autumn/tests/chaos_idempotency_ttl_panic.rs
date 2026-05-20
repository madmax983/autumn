use autumn_web::idempotency::{MemoryIdempotencyStore, IdempotencyStore, IdempotencyRecord};
use std::time::Duration;

#[test]
fn test_idempotency_ttl_panic() {
    let store = MemoryIdempotencyStore::new(Duration::from_secs(60));
    let record = IdempotencyRecord {
        status: 200,
        headers: vec![],
        body: vec![],
        metadata: vec![],
    };
    store.set("key", record, vec![], Duration::MAX);
}
