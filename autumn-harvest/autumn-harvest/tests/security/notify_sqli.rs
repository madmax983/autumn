use autumn_harvest::notify::notify_task_enqueued;
use uuid::Uuid;

#[tokio::test]
async fn test_notify_sql_injection_poc() {
    use diesel_async::{AsyncPgConnection, RunQueryDsl, AsyncConnection};
    use std::env;

    let db_url = if let Ok(url) = env::var("POSTGRES_URL") {
        url
    } else {
        "postgres://postgres:postgres@localhost:5432/postgres".to_string()
    };

    let mut conn = match AsyncPgConnection::establish(&db_url).await {
        Ok(c) => c,
        Err(_) => return,
    };

    diesel::sql_query("CREATE TABLE IF EXISTS dummy_test_table (id INT)")
        .execute(&mut conn)
        .await
        .unwrap();

    let malicious_queue_name = "test; DROP TABLE dummy_test_table; --";

    let task_id = Uuid::new_v4();
    let _ = notify_task_enqueued(&mut conn, malicious_queue_name, task_id).await;

    let table_exists = diesel::sql_query("SELECT * FROM dummy_test_table")
        .execute(&mut conn)
        .await
        .is_ok();

    assert!(table_exists, "[ERIS-REGRESSION] SQL Injection vulnerability regressed: arbitrary DROP TABLE executed via notify_task_enqueued");

    // Clean up
    diesel::sql_query("DROP TABLE IF EXISTS dummy_test_table")
        .execute(&mut conn)
        .await
        .unwrap();
}
