use autumn_web::auth::{ApiTokenStore, InMemoryApiTokenStore};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_memory_api_token_concurrency_stress() {
    let store = Arc::new(InMemoryApiTokenStore::default());
    let mut handles = vec![];

    for i in 0..100 {
        let store_clone = store.clone();
        handles.push(tokio::spawn(async move {
            let user = format!("user:{i}");
            let token = store_clone.issue(&user).await.unwrap();
            let verified = store_clone.verify(&token).await.unwrap();
            assert_eq!(verified, Some(user));
            store_clone.revoke(&token).await.unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}
