use autumn_web::session::MemoryStore;
use autumn_web::session::SessionStore;
use loom::thread;
use std::collections::HashMap;

#[test]
fn session_concurrent_mutations() {
    loom::model(|| {
        let store = MemoryStore::new();
        let store = std::sync::Arc::new(store);

        let s1 = store.clone();
        let t1 = thread::spawn(move || {
            let mut data = HashMap::new();
            data.insert("key".to_string(), "val1".to_string());
            let _ = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(s1.save("test_id", data));
        });

        let s2 = store.clone();
        let t2 = thread::spawn(move || {
            let mut data = HashMap::new();
            data.insert("key".to_string(), "val2".to_string());
            let _ = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(s2.save("test_id", data));
        });

        t1.join().unwrap();
        t2.join().unwrap();

        let _ = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(store.load("test_id"));
    });
}
