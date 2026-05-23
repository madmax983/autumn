use autumn_web::idempotency::{IdempotencyRecord, IdempotencyStore, MemoryIdempotencyStore};
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #[test]
    fn test_memory_idempotency_ttl_fuzzing(ttl_secs in 0..=(u64::MAX - 4), ttl_nanos in any::<u32>()) {
        let store = MemoryIdempotencyStore::new(Duration::from_secs(60));
        let record = IdempotencyRecord {
            status: 200,
            headers: vec![],
            body: vec![],
            metadata: Vec::default(),
        };
        let ttl = Duration::new(ttl_secs, ttl_nanos);
        store.set("test_key", record, vec![], ttl);
    }
}

proptest! {
    #[test]
    fn test_memory_idempotency_try_lock_ttl_fuzzing(ttl_secs in 0..=(u64::MAX - 4), ttl_nanos in any::<u32>()) {
        let store = MemoryIdempotencyStore::new(Duration::from_secs(60));
        let ttl = Duration::new(ttl_secs, ttl_nanos);
        store.try_lock("test_key", ttl);
    }
}
