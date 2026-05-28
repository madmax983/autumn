use autumn_web::auth::{ApiTokenStore, InMemoryApiTokenStore};
use loom::sync::Arc;
use loom::thread;
use std::future::Future;

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn concurrent_token_issue() {
    loom::model(|| {
        let store = Arc::new(InMemoryApiTokenStore::default());

        let s1 = store.clone();
        let t1 = thread::spawn(move || {
            block_on(s1.issue("user:1")).unwrap();
        });

        let s2 = store;
        let t2 = thread::spawn(move || {
            block_on(s2.issue("user:2")).unwrap();
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}
