#![cfg(feature = "db")]

use autumn_web::tenancy::with_tenant;
use diesel_async::RunQueryDsl;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

mod schema {
    autumn_web::reexports::diesel::table! {
        tenant_posts (id) {
            id -> Int8,
            title -> Text,
            tenant_id -> Text,
        }
    }
}

use schema::tenant_posts;

#[autumn_web::model(table = "tenant_posts")]
pub struct TenantPost {
    #[id]
    pub id: i64,
    pub title: String,
    #[default]
    pub tenant_id: String,
}

#[autumn_web::repository(TenantPost, table = "tenant_posts", tenant_scoped)]
pub trait TenantPostRepository {
    fn find_by_title(title: String) -> Vec<TenantPost>;
}

// Helper to set up the DB table.
async fn setup_db(
    pool: &autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
        autumn_web::reexports::diesel_async::AsyncPgConnection,
    >,
) {
    let mut conn = pool.get().await.unwrap();
    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS tenant_posts (
            id BIGSERIAL PRIMARY KEY,
            title TEXT NOT NULL,
            tenant_id TEXT NOT NULL
        )",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    diesel::sql_query("TRUNCATE tenant_posts RESTART IDENTITY")
        .execute(&mut *conn)
        .await
        .unwrap();
}

// 1. Test standard/derived CRUD operations under a specific tenant context
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn test_tenant_scoping_isolation() {
    let container = testcontainers_modules::postgres::Postgres::default()
        .start()
        .await
        .expect("failed to start Postgres container");
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    let manager = diesel_async::pooled_connection::AsyncDieselConnectionManager::<
        diesel_async::AsyncPgConnection,
    >::new(&url);
    let pool = diesel_async::pooled_connection::deadpool::Pool::builder(manager)
        .max_size(5)
        .build()
        .unwrap();

    setup_db(&pool).await;

    let repo = PgTenantPostRepository {
        pool,
        across_tenants: false,
    };

    // Save record for tenant A
    let post_a = with_tenant("tenant-a".to_string(), async {
        repo.save(&NewTenantPost {
            title: "Post A".to_string(),
        })
        .await
        .unwrap()
    })
    .await;
    assert_eq!(post_a.tenant_id, "tenant-a");

    // Save record for tenant B
    let post_b = with_tenant("tenant-b".to_string(), async {
        repo.save(&NewTenantPost {
            title: "Post B".to_string(),
        })
        .await
        .unwrap()
    })
    .await;
    assert_eq!(post_b.tenant_id, "tenant-b");

    // Assert tenant A can only read tenant A's post
    with_tenant("tenant-a".to_string(), async {
        let all = repo.find_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Post A");

        let found = repo.find_by_id(post_a.id).await.unwrap().unwrap();
        assert_eq!(found.title, "Post A");

        let found_by_title = repo.find_by_title("Post A".to_string()).await.unwrap();
        assert_eq!(found_by_title.len(), 1);

        let not_found = repo.find_by_id(post_b.id).await.unwrap();
        assert!(not_found.is_none());

        let exists = repo.exists_by_id(post_a.id).await.unwrap();
        assert!(exists);
        let exists_b = repo.exists_by_id(post_b.id).await.unwrap();
        assert!(!exists_b);
    })
    .await;
}

// 2. Test that calling repository methods without a tenant context throws an error
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn test_unscoped_query_without_context_fails() {
    let container = testcontainers_modules::postgres::Postgres::default()
        .start()
        .await
        .expect("failed to start Postgres container");
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    let manager = diesel_async::pooled_connection::AsyncDieselConnectionManager::<
        diesel_async::AsyncPgConnection,
    >::new(&url);
    let pool = diesel_async::pooled_connection::deadpool::Pool::builder(manager)
        .max_size(5)
        .build()
        .unwrap();

    setup_db(&pool).await;
    let repo = PgTenantPostRepository {
        pool,
        across_tenants: false,
    };

    // Scoped methods should fail when run unscoped without context
    let result = repo.find_all().await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("no tenant context was established"),
        "Expected tenant context error, got: {err}"
    );
}

// 3. Test that the across_tenants() escape hatch works and bypasses scoping
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn test_escape_hatch_across_tenants() {
    let container = testcontainers_modules::postgres::Postgres::default()
        .start()
        .await
        .expect("failed to start Postgres container");
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    let manager = diesel_async::pooled_connection::AsyncDieselConnectionManager::<
        diesel_async::AsyncPgConnection,
    >::new(&url);
    let pool = diesel_async::pooled_connection::deadpool::Pool::builder(manager)
        .max_size(5)
        .build()
        .unwrap();

    setup_db(&pool).await;
    let repo = PgTenantPostRepository {
        pool,
        across_tenants: false,
    };

    with_tenant("tenant-a".to_string(), async {
        repo.save(&NewTenantPost {
            title: "Post A".to_string(),
        })
        .await
        .unwrap();
    })
    .await;
    with_tenant("tenant-b".to_string(), async {
        repo.save(&NewTenantPost {
            title: "Post B".to_string(),
        })
        .await
        .unwrap();
    })
    .await;

    // Now read across all tenants
    let all = repo.across_tenants().find_all().await.unwrap();
    assert_eq!(all.len(), 2);
}
