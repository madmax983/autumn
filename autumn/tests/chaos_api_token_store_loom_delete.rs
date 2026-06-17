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
fn concurrent_token_revoke() {
    loom::model(|| {
        let store = Arc::new(InMemoryApiTokenStore::default());
        let token = block_on(store.issue("user:1")).unwrap();

        let s1 = store.clone();
        let t1 = token.clone();
        let th1 = thread::spawn(move || {
            block_on(s1.revoke(&t1)).unwrap();
        });

        let s2 = store.clone();
        let t2 = token.clone();
        let th2 = thread::spawn(move || {
            block_on(s2.revoke(&t2)).unwrap();
        });

        th1.join().unwrap();
        th2.join().unwrap();

        let verified = block_on(store.verify(&token)).unwrap();
        assert_eq!(verified, None);
    });
}
