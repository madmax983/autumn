use autumn_harvest::notify::notify_task_enqueued;
use diesel_async::{AsyncConnection, AsyncPgConnection};
use uuid::Uuid;

#[tokio::test]
async fn test_notify_sql_injection() {
    let url = std::env::var("POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_string());

    // Attempt to connect. If it fails (e.g. no local postgres), we skip the test.
    let mut conn = match AsyncPgConnection::establish(&url).await {
        Ok(c) => c,
        Err(_) => {
            println!("Skipping test_notify_sql_injection: Postgres not available");
            return;
        }
    };

    // Before fix, this would execute pg_sleep(2)
    let malicious_queue = "test', 'payload'); SELECT pg_sleep(2); --";

    let task_id = Uuid::new_v4();
    let start = std::time::Instant::now();
    let result = notify_task_enqueued(&mut conn, malicious_queue, task_id).await;
    let elapsed = start.elapsed();

    assert!(
        result.is_ok(),
        "Failed to execute pg_notify with malicious queue name"
    );
    assert!(
        elapsed.as_secs() < 1,
        "SQL Injection succeeded! The query slept."
    );
}
