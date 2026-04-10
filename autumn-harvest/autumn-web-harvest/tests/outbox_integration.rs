use autumn_web::AppState;
use autumn_web::config::DatabaseConfig;
use autumn_web_harvest::{
    HarvestDbPool, WorkflowStartRequest, drain_workflow_start_outbox_once,
    enqueue_workflow_start_outbox,
};
use diesel_async::AsyncConnection;
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use diesel_async::SimpleAsyncConnection;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use uuid::Uuid;

const OUTBOX_INIT_SQL: &str =
    include_str!("../migrations/20260409010000_harvest_workflow_outbox/up.sql");
const HARVEST_INIT_SQL: &str =
    include_str!("../../autumn-harvest/migrations/20260409000000_harvest_initial/up.sql");

#[tokio::test]
async fn framework_outbox_delivers_to_split_harvest_store() {
    let (state, mut app_conn, _harvest_url, _container) = setup_split_test_databases(true).await;

    let request = WorkflowStartRequest {
        workflow_name: "user_onboarding".to_string(),
        workflow_id: "user-onboarding:42".to_string(),
        queue_name: "default".to_string(),
        input: serde_json::json!({
            "user_id": 42,
            "username": "ferris",
        }),
        memo: Some(serde_json::json!({
            "kind": "user_onboarding",
            "user_id": 42,
        })),
        search_attrs: Some(serde_json::json!({
            "user_id": 42,
            "username": "ferris",
        })),
    };

    enqueue_workflow_start_outbox(&mut app_conn, &request)
        .await
        .expect("outbox row should persist in the app database");

    let delivered = drain_workflow_start_outbox_once(&state, 32)
        .await
        .expect("outbox drain should succeed");
    assert_eq!(delivered, 1, "one row should be delivered");
}

#[tokio::test]
async fn framework_outbox_retries_failed_delivery() {
    let (state, mut app_conn, _harvest_url, _container) = setup_split_test_databases(false).await;

    let request = WorkflowStartRequest {
        workflow_name: "post_publication".to_string(),
        workflow_id: "post-publication:99".to_string(),
        queue_name: "default".to_string(),
        input: serde_json::json!({
            "post_id": 99,
            "title": "Ferris arrives",
        }),
        memo: Some(serde_json::json!({
            "kind": "post_publication",
            "post_id": 99,
        })),
        search_attrs: Some(serde_json::json!({
            "post_id": 99,
            "post_slug": "ferris-arrives",
        })),
    };

    enqueue_workflow_start_outbox(&mut app_conn, &request)
        .await
        .expect("outbox row should persist even when delivery later fails");

    let delivered = drain_workflow_start_outbox_once(&state, 32)
        .await
        .expect("outbox drain should record failure without crashing");
    assert_eq!(delivered, 0, "failed delivery should not report success");
}

async fn setup_split_test_databases(
    apply_harvest_migrations: bool,
) -> (
    AppState,
    AsyncPgConnection,
    String,
    ContainerAsync<Postgres>,
) {
    let container = Postgres::default()
        .start()
        .await
        .expect("failed to start Postgres container");

    let host = container
        .get_host()
        .await
        .expect("failed to get container host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to get container port");
    let admin_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let app_db_name = format!("framework_outbox_app_{}", Uuid::new_v4().simple());
    let harvest_db_name = format!("framework_outbox_harvest_{}", Uuid::new_v4().simple());

    let mut admin_conn = <AsyncPgConnection as AsyncConnection>::establish(&admin_url)
        .await
        .expect("failed to connect to admin database");
    diesel::sql_query(format!("CREATE DATABASE {app_db_name}"))
        .execute(&mut admin_conn)
        .await
        .expect("failed to create app database");
    diesel::sql_query(format!("CREATE DATABASE {harvest_db_name}"))
        .execute(&mut admin_conn)
        .await
        .expect("failed to create harvest database");

    let app_url = format!("postgres://postgres:postgres@{host}:{port}/{app_db_name}");
    let harvest_url = format!("postgres://postgres:postgres@{host}:{port}/{harvest_db_name}");

    let mut app_conn = <AsyncPgConnection as AsyncConnection>::establish(&app_url)
        .await
        .expect("failed to connect to app database");
    app_conn
        .batch_execute(OUTBOX_INIT_SQL)
        .await
        .expect("failed to apply app outbox migration");

    if apply_harvest_migrations {
        let mut harvest_conn = <AsyncPgConnection as AsyncConnection>::establish(&harvest_url)
            .await
            .expect("failed to connect to harvest database");
        harvest_conn
            .batch_execute(HARVEST_INIT_SQL)
            .await
            .expect("failed to apply harvest migrations");
    }

    let app_pool = build_pool(&app_url);
    let harvest_pool = build_pool(&harvest_url);
    let state = AppState::for_test().with_pool(app_pool);
    state.insert_extension(HarvestDbPool::from(harvest_pool));

    (state, app_conn, harvest_url, container)
}

fn build_pool(
    database_url: &str,
) -> diesel_async::pooled_connection::deadpool::Pool<AsyncPgConnection> {
    autumn_web::db::create_pool(&DatabaseConfig {
        url: Some(database_url.to_owned()),
        pool_size: 4,
        ..DatabaseConfig::default()
    })
    .expect("failed to build pool config")
    .expect("database url should create a pool")
}
