use autumn_web::idempotency::{IdempotencyRecord, IdempotencyStore, MemoryIdempotencyStore};
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #[test]
    fn test_memory_idempotency_store_set_ttl_panic(ttl_secs in any::<u64>()) {
        let store = MemoryIdempotencyStore::new(Duration::from_secs(60));
        let record = IdempotencyRecord {
            status: 200,
            headers: vec![],
            body: vec![],
            metadata: vec![],
        };
        store.set("key", record, vec![], Duration::from_secs(ttl_secs));
    }
}
