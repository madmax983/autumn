use autumn_harvest::notify::notify_task_enqueued;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use uuid::Uuid;

#[tokio::test]
async fn test_notify_sql_injection_poc() {
    let container = Postgres::default()
        .start()
        .await
        .expect("failed to start Postgres container");

    let host = container.get_host().await.expect("failed to get host");

    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to get port");

    let db_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    let mut conn = AsyncPgConnection::establish(&db_url).await.unwrap();

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

    assert!(
        table_exists,
        "[ERIS-REGRESSION] SQL Injection vulnerability regressed: arbitrary DROP TABLE executed via notify_task_enqueued"
    );

    // Clean up
    diesel::sql_query("DROP TABLE IF EXISTS dummy_test_table")
        .execute(&mut conn)
        .await
        .unwrap();
}
