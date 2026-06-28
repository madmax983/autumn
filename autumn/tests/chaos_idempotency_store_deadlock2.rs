use autumn_web::idempotency::{IdempotencyStore, MemoryIdempotencyStore};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn idempotency_store_deadlock() {
    let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));
    let key = "test_key";

    let s1 = store.clone();
    let t1 = thread::spawn(move || {
        for _ in 0..1000 {
            s1.try_lock(key, Duration::from_secs(5));
        }
    });

    let s2 = store.clone();
    let t2 = thread::spawn(move || {
        for _ in 0..1000 {
            s2.get(key);
        }
    });

    t1.join().unwrap();
    t2.join().unwrap();
}
