//! Integration tests for transactional DB test isolation.
//!
//! Run with:
//!
//!     cargo test --test transactional_test_integration -- --include-ignored

#[cfg(all(feature = "db", feature = "test-support"))]
mod transactional_tests {
    use autumn_web::prelude::*;
    use autumn_web::test::{TestApp, TestDb};
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;

    // ── Schema ─────────────────────────────────────────────────

    diesel::table! {
        transactional_items (id) {
            id -> Int8,
            name -> Text,
        }
    }

    #[derive(Debug, Queryable, Selectable, serde::Serialize)]
    #[diesel(table_name = transactional_items)]
    struct Item {
        pub id: i64,
        pub name: String,
    }

    #[derive(Debug, Insertable, serde::Deserialize)]
    #[diesel(table_name = transactional_items)]
    struct NewItem {
        pub name: String,
    }

    // ── Handlers ───────────────────────────────────────────────

    #[get("/items")]
    async fn list_items(mut db: Db) -> AutumnResult<Json<Vec<Item>>> {
        let items = transactional_items::table
            .select(Item::as_select())
            .load(&mut *db)
            .await?;
        Ok(Json(items))
    }

    #[post("/items")]
    async fn create_item(
        mut db: Db,
        Json(new_item): Json<NewItem>,
    ) -> AutumnResult<(axum::http::StatusCode, Json<Item>)> {
        let item = diesel::insert_into(transactional_items::table)
            .values(&new_item)
            .returning(Item::as_returning())
            .get_result(&mut *db)
            .await?;
        Ok((axum::http::StatusCode::CREATED, Json(item)))
    }

    // ── Setup ──────────────────────────────────────────────────

    static SETUP_CELL: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

    async fn setup_table(db: &TestDb) {
        SETUP_CELL
            .get_or_init(|| async {
                db.execute_sql(
                    "CREATE TABLE IF NOT EXISTS transactional_items (
                    id BIGSERIAL PRIMARY KEY,
                    name TEXT NOT NULL
                )",
                )
                .await;
            })
            .await;
    }

    // ── Tests ──────────────────────────────────────────────────

    #[tokio::test]
    #[ignore = "requires Docker (testcontainers)"]
    async fn test_1_insert_without_cleanup() {
        let db = TestDb::shared().await;
        setup_table(db).await;

        let client = TestApp::new()
            .routes(routes![list_items, create_item])
            .with_transactional_db(db.url())
            .build();

        // 1. Initially empty
        client
            .get("/items")
            .send()
            .await
            .assert_ok()
            .assert_body_eq("[]");

        // 2. Insert item
        client
            .post("/items")
            .json(&serde_json::json!({"name": "Persisted Item"}))
            .send()
            .await
            .assert_status(201);

        // 3. Verify it is visible inside the same test session
        client
            .get("/items")
            .send()
            .await
            .assert_ok()
            .assert_json::<Vec<serde_json::Value>, _>(|items| {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0]["name"], "Persisted Item");
            });

        // We finish the test WITHOUT running any TRUNCATE or DELETE.
        // Under transactional isolation, this transaction should be rolled back!
    }

    #[tokio::test]
    #[ignore = "requires Docker (testcontainers)"]
    async fn test_2_verify_isolation() {
        let db = TestDb::shared().await;
        setup_table(db).await;

        let client = TestApp::new()
            .routes(routes![list_items])
            .with_transactional_db(db.url())
            .build();

        // Verify that the table is completely empty!
        // This proves that test_1's uncommitted transaction was rolled back
        // and did not leak any state to this test.
        client
            .get("/items")
            .send()
            .await
            .assert_ok()
            .assert_body_eq("[]");
    }
}
