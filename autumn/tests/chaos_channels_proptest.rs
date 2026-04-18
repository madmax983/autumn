#![cfg(feature = "ws")]

use autumn_web::channels::Channels;
use proptest::prelude::*;
use std::sync::Arc;

proptest! {
    #[test]
    fn test_channels_capacity_fuzzing(capacity in any::<usize>()) {
        let channels = Channels::new(capacity);

        // This should never panic on any capacity (see explicit zero test)
        let tx = channels.sender("test_channel");

        // Edge case: Sending with no subscribers should cleanly error, not panic
        let res = tx.send("test");
        prop_assert!(res.is_err());

        // Exercise capacity: Adding a subscriber should make the send successful
        // and actually rely on the generated capacity.
        let _rx = channels.subscribe("test_channel");
        prop_assert!(tx.send("test_with_subscriber").is_ok());
    }
}

#[tokio::test]
async fn test_channels_zero_capacity_regression() {
    let channels = Arc::new(Channels::new(0));
    let mut tasks = vec![];

    // 10 concurrent writers overfilling the 1-capacity buffer
    for i in 0..10 {
        let channels = Arc::clone(&channels);
        tasks.push(tokio::spawn(async move {
            let tx = channels.sender("chaos_channel");
            for j in 0..100 {
                let _ = tx.send(format!("msg_{i}_{j}"));
                tokio::task::yield_now().await;
            }
        }));
    }

    // 10 concurrent readers experiencing lagged errors
    for _ in 0..10 {
        let channels = Arc::clone(&channels);
        tasks.push(tokio::spawn(async move {
            let mut rx = channels.subscribe("chaos_channel");
            let mut lagged = 0;

            for _ in 0..100 {
                if let Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) = rx.recv().await {
                    lagged += 1;
                }
            }

            // Buffer size is 1, writers send 1000 messages total.
            // Readers will definitely fall behind and experience Lagged errors.
            assert!(lagged > 0);
        }));
    }

    for task in tasks {
        task.await.expect("Task panicked");
    }
}
