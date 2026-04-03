use autumn_harvest::notify::{QueueListener, notify_task_enqueued, queue_channel};
use diesel_async::{AsyncConnection, AsyncPgConnection};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[tokio::test]
async fn test_eris_notify_sql_injection() {
    let database_url = std::env::var("AUTUMN_DATABASE__URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/autumn_test".to_string());

    let mut conn = AsyncPgConnection::establish(&database_url)
        .await
        .expect("connect");

    let task_id = Uuid::new_v4();

    // The `queue_channel` function replaces hyphens with underscores, but does not escape double quotes.
    // If the queue name is wrapped in quotes, it can break out of the identifier and execute arbitrary SQL.
    let malicious_queue_name = "\" ; SELECT 1 ; -- ";

    // The query string will be: `NOTIFY harvest_queue_" ; SELECT 1 ; -- , '{"task_id":"..."}'`
    // Which is `NOTIFY harvest_queue_" ;` followed by `SELECT 1 ;` and then a comment.
    // Wait, the double quotes need to be paired to be valid identifier in PostgreSQL.
    // Postgres allows `NOTIFY "my_channel"`, so if the channel name starts with `harvest_queue_`,
    // wait, `NOTIFY harvest_queue_" ; SELECT 1 ; -- ` is a syntax error because `harvest_queue_` is unquoted.
    // Postgres requires the whole identifier to be quoted if it has special characters.
    // Let's look at `sql = format!("NOTIFY {channel}, '{payload}'");`
    // If `channel` is `a; SELECT pg_sleep(1); --`, it becomes `NOTIFY a; SELECT pg_sleep(1); --, '...'`.
    // The query parses `NOTIFY a` and then `SELECT pg_sleep(1)`.

    let malicious_queue_name2 = "a; SELECT 1; --";

    let result = notify_task_enqueued(&mut conn, malicious_queue_name2, task_id).await;

    // In a vulnerable system, this query will execute successfully as two statements.
    // In a fixed system using `pg_notify($1, $2)`, it will be treated as a single parameter,
    // which is perfectly valid and won't throw a syntax error, but the channel name will literally be
    // `harvest_queue_a; SELECT 1; --`.
    // Let's construct a payload that will ERROR if injected, but succeed if parameterized.
    // Let's use `a; SYNTAX ERROR HERE; --`
    let error_queue_name = "a; SELECT * FROM nonexistent_table_that_definitely_fails; --";

    let result2 = notify_task_enqueued(&mut conn, error_queue_name, task_id).await;

    // If it's vulnerable, it will return a Database error (relation "nonexistent_table..." does not exist)
    // If it's fixed (parameterized), it will just notify the channel `harvest_queue_a; SELECT * ...` and succeed!
    // Wait, let's write the test to assert that it SUCCEEDS. This proves it is parameterized.
    assert!(result2.is_ok(), "The payload was parameterized properly, but instead it failed: {:?}", result2.err());

    // For LISTEN:
    let result3 = QueueListener::connect(&database_url, &[error_queue_name.to_string()]).await;
    assert!(result3.is_ok(), "The LISTEN command was quoted properly, but instead it failed: {:?}", result3.err());
}
