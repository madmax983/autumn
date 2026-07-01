use autumn_web::idempotency::*;
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #[test]
    fn test_memory_store_panic_on_huge_ttl(ttl_secs in any::<u64>(), ttl_nanos in any::<u32>()) {
        let store = MemoryIdempotencyStore::new(Duration::from_secs(3600));
        let record = IdempotencyRecord {
            status: 200,
            headers: vec![],
            body: vec![],
            metadata: vec![],
        };

        let ttl = Duration::new(ttl_secs, ttl_nanos % 1_000_000_000);
        store.set("test_key", record.clone(), vec![], ttl);
        store.try_lock("test_key_2", ttl);
    }
}
