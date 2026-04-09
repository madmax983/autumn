import os
import sys

def process(path, func):
    with open(path, 'r') as f:
        content = f.read()

    res = func(content)

    with open(path, 'w') as f:
        f.write(res)

def fix_web_harvest(content):
    old = """async fn setup_test_database_url() -> (String, ContainerAsync<Postgres>) {
    let container = Postgres::default()
        .with_init_sql(INIT_SQL.to_string().into_bytes())
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
    let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    (database_url, container)
}"""
    new = """async fn setup_test_database_url() -> (String, Option<ContainerAsync<Postgres>>) {
    if let Ok(url) = std::env::var("POSTGRES_URL") {
        return (url, None);
    }
    let container = Postgres::default()
        .with_init_sql(INIT_SQL.to_string().into_bytes())
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
    let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    (database_url, Some(container))
}"""
    # Replace the signature
    content = content.replace(old, new)

    # We must also change the callers
    # Usually it's `let (database_url, _container) = setup_test_database_url().await;`
    # or `let (database_url, container) = setup_test_database_url().await;`
    # Let's search for those assignments

    # In `api_scheduler_integration.rs` it is typically:
    # let (database_url, _container) = setup_test_database_url().await;

    return content

process('autumn-harvest/autumn-web-harvest/tests/api_scheduler_integration.rs', fix_web_harvest)
