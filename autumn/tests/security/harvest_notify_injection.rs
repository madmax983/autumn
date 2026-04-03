use autumn_harvest::notify::{QueueListener, notify_task_enqueued};
use autumn_harvest::pool::compute_pool_sizes;
use diesel_async::{AsyncConnection, AsyncPgConnection};
use uuid::Uuid;

#[tokio::test]
async fn test_eris_notify_sql_injection() {
    let database_url = std::env::var("AUTUMN_DATABASE__URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/autumn_test".to_string());

    let mut conn = AsyncPgConnection::establish(&database_url)
        .await
        .expect("connect");

    let malicious_queue_name = "\"; SELECT pg_sleep(1); --";
    let task_id = Uuid::new_v4();

    // If it's vulnerable to SQL injection, notify_task_enqueued will fail or execute the payload.
    // In our case, the SQL will be: `NOTIFY harvest_queue_"; SELECT pg_sleep(1); --, '{...}'`
    // Wait, the double quotes in malicious_queue_name will break the SQL if it isn't properly quoted.
    // But since `NOTIFY channel` doesn't require quotes unless it's a quoted identifier,
    // if we pass `; SELECT 1; --`, it will become:
    // NOTIFY harvest_queue_; SELECT 1; --, '{"task_id":"..."}'
    // Let's use `a; SELECT 1; --`

    let malicious_queue_name2 = "a; SELECT 1; --";
    let result = notify_task_enqueued(&mut conn, malicious_queue_name2, task_id).await;

    // We expect this to fail due to the syntax error if not parameterized/quoted properly,
    // OR we can demonstrate that the query executed successfully by checking that it didn't error.
    // But actually if it's "NOTIFY harvest_queue_a; SELECT 1; --, '...'", the -- comments out the rest,
    // so it executes `NOTIFY harvest_queue_a; SELECT 1;` which is valid SQL and succeeds!
    // A secure implementation would escape or parameterize it, which would treat the whole thing as a single channel name
    // and would either succeed as a channel name or fail. But if it's injected, the query string is split.

    assert!(result.is_ok(), "The payload was injected and the query ran successfully as multiple statements");

    // Wait, let's also test the QueueListener
    let listener_result = QueueListener::connect(&database_url, &[malicious_queue_name2.to_string()]).await;
    assert!(listener_result.is_ok(), "LISTEN was injected and ran successfully");
}
