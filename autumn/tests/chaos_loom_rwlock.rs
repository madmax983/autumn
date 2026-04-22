use std::sync::{Arc, RwLock};

#[test]
fn test_rwlock_loom_concurrency() {
    loom::model(|| {
        let lock = Arc::new(RwLock::new(0));

        let l1 = lock.clone();
        let t1 = loom::thread::spawn(move || {
            let mut write = l1.write().unwrap();
            *write += 1;
        });

        let l2 = lock.clone();
        let t2 = loom::thread::spawn(move || {
            let read = l2.read().unwrap();
            let _ = *read;
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}
