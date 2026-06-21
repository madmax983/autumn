use autumn_web::webhook::{InMemoryWebhookReplayStore, WebhookReplayStore};
use std::time::{Duration, SystemTime};

#[test]
fn test_webhook_cleanup_vulnerability() {
    let store = InMemoryWebhookReplayStore::default();

    // 1. Insert a legitimate entry.
    let legitimate_key = "legitimate_key";
    let now = SystemTime::now();
    let window = Duration::from_secs(60);

    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(store.check_and_insert(&legitimate_key, now, window))
        .unwrap();

    // legitimate_key expires at now + 60s

    // 2. An attacker sends a request with an artificially high timestamp (future).
    // This triggers cleanup because we do it 128 times, or just enough to hit the limit.
    let future_time = now + Duration::from_secs(100);
    for i in 0..128 {
        let _ = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(store.check_and_insert(&format!("attacker_key_{}", i), future_time, window))
            .unwrap();
    }

    // 3. Check if the legitimate entry is still there.
    // We send another request for legitimate_key at `now`.
    let reinserted = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(store.check_and_insert(&legitimate_key, now, window))
        .unwrap();

    // If it was wrongfully evicted, `reinserted` will be true!
    // Since we fixed it, it should NOT be true.
    assert!(
        !reinserted,
        "The legitimate entry was wrongfully evicted by the attacker's future timestamp!"
    );
}
