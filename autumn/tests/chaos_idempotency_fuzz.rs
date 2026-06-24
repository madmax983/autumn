use autumn_web::idempotency::{IdempotencyRecord, IdempotencyStore, MemoryIdempotencyStore};
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10))]
    #[test]
    fn idempotency_memory_store_ttl_overflow(ttl_secs in (u64::MAX - 1000)..u64::MAX) {
        let store = MemoryIdempotencyStore::new(Duration::from_secs(60));
        let record = IdempotencyRecord {
            status: 200,
            headers: vec![],
            body: vec![],
            metadata: vec![],
        };
        store.set("test-key", record, vec![], Duration::from_secs(ttl_secs));
    }
}
