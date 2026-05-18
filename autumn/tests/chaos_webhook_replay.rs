use autumn_web::webhook::{InMemoryWebhookReplayStore, WebhookReplayStore};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webhook_replay_concurrency_stress() {
    let store = Arc::new(InMemoryWebhookReplayStore::default());
    let mut handles = vec![];

    for i in 0..1000 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let key = format!("event-{i}");
            let time = SystemTime::now();
            let window = Duration::from_secs(60);
            s.check_and_insert(&key, time, window).await.unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}
