use autumn_web::idempotency::IdempotencyRecord;
use autumn_web::idempotency::IdempotencyStore;
use autumn_web::idempotency::MemoryIdempotencyStore;
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #[test]
    fn test_idempotency_ttl_fuzz(ttl in proptest::num::u64::ANY) {
        let store = MemoryIdempotencyStore::new(Duration::from_secs(3600));
        let record = IdempotencyRecord {
            status: 200,
            headers: vec![],
            body: vec![],
            metadata: vec![],
        };

        let d = Duration::from_secs(ttl);
        store.set("key", record, vec![], d);
        store.try_lock("key", d);
    }
}
