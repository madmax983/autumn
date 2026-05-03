//! Integration tests for `examples/todo-app` (the tutorial reference app).
//!
//! Mirrors the code shown in `docs/guide/tutorial/11-testing.md`.
//!
//! # Running
//!
//! ```text
//! cargo test -p todo-app                                              # smoke tests (instant)
//! cargo test -p todo-app -- --include-ignored --test-threads=1       # DB tests (needs Docker)
//! ```
//!
//! `--test-threads=1` is required when running multiple DB-backed ignored
//! tests: each test truncates the shared table, so concurrent execution would
//! cause data races. With a single DB test the flag is optional, but keeping
//! it explicit avoids surprises as the suite grows.

use autumn_web::prelude::*;
use autumn_web::test::{TestApp, TestDb};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};

// ── Inline schema (matches todo-app migrations) ────────────────────────────

diesel::table! {
    todos (id) {
        id -> Int8,
        title -> Text,
        completed -> Bool,
    }
}

// ── Local model types ──────────────────────────────────────────────────────

#[derive(Debug, Queryable, Selectable, Serialize)]
#[diesel(table_name = todos)]
struct Todo {
    id: i64,
    title: String,
    completed: bool,
}

#[derive(Debug, Insertable, Deserialize)]
#[diesel(table_name = todos)]
struct NewTodo {
    title: String,
    #[serde(default)]
    completed: bool,
}

// ── Todo-shaped route handlers (mirror routes/api.rs) ─────────────────────

/// Return all todos as a JSON array.
#[get("/api/todos")]
async fn list_todos(mut db: Db) -> AutumnResult<Json<Vec<Todo>>> {
    let all = todos::table
        .select(Todo::as_select())
        .load(&mut *db)
        .await?;
    Ok(Json(all))
}

/// Create a new todo from a JSON body.
#[post("/api/todos")]
async fn create_todo(mut db: Db, Json(body): Json<NewTodo>) -> AutumnResult<Json<Todo>> {
    let created = diesel::insert_into(todos::table)
        .values(&body)
        .returning(Todo::as_returning())
        .get_result(&mut *db)
        .await?;
    Ok(Json(created))
}

// ── DB setup helper ────────────────────────────────────────────────────────

async fn setup_todos_table()
-> diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection> {
    let db = TestDb::shared().await;
    db.execute_sql(
        "CREATE TABLE IF NOT EXISTS todos (
            id        BIGSERIAL PRIMARY KEY,
            title     TEXT    NOT NULL,
            completed BOOLEAN NOT NULL DEFAULT false
        )",
    )
    .await;
    db.execute_sql("TRUNCATE todos RESTART IDENTITY").await;
    db.pool()
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// **Smoke test** — runs instantly, no Docker required.
///
/// Proves that `TestApp` boots the full Autumn middleware pipeline in-process
/// and routes requests correctly. `X-Request-Id` is Autumn-specific: it is
/// added by `RequestIdLayer` on every response, confirming the middleware
/// stack ran end-to-end.
#[tokio::test]
async fn smoke_test_get_returns_200_with_autumn_request_id() {
    #[get("/ping")]
    async fn ping() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![ping]).build();

    let resp = client.get("/ping").send().await;

    resp.assert_ok().assert_body_eq("pong");
    assert!(
        resp.header("x-request-id").is_some(),
        "Autumn's RequestIdLayer must attach X-Request-Id to every response"
    );
}

/// **DB round-trip** — requires Docker (testcontainers starts Postgres).
///
/// Demonstrates the three-step Autumn integration test pattern:
/// 1. Boot a shared Postgres container and run schema setup.
/// 2. `POST /api/todos` — write a row, assert the returned JSON.
/// 3. `GET  /api/todos` — read the list, confirm the write persisted.
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn create_todo_round_trip() {
    let pool = setup_todos_table().await;
    let client = TestApp::new()
        .routes(routes![list_todos, create_todo])
        .with_db(pool)
        .build();

    // Initially empty.
    client
        .get("/api/todos")
        .send()
        .await
        .assert_ok()
        .assert_body_eq("[]");

    // Create a todo (DB write).
    let resp = client
        .post("/api/todos")
        .json(&serde_json::json!({"title": "Learn Autumn testing"}))
        .send()
        .await;

    resp.assert_ok()
        .assert_header_contains("content-type", "application/json")
        .assert_json::<serde_json::Value, _>(|todo| {
            assert_eq!(todo["title"], "Learn Autumn testing");
            assert_eq!(todo["completed"], false);
            assert!(todo["id"].as_i64().unwrap() > 0, "DB must assign a real ID");
        });

    // Confirm the write persisted (DB read).
    client
        .get("/api/todos")
        .send()
        .await
        .assert_ok()
        .assert_json::<Vec<serde_json::Value>, _>(|todos| {
            assert_eq!(todos.len(), 1, "exactly one todo after creation");
            assert_eq!(todos[0]["title"], "Learn Autumn testing");
        });
}

// ── Factory tests ─────────────────────────────────────────────────────────────
//
// Uses `#[autumn_web::model]` to exercise the generated factory builder.
// Unique table name (`factory_todo_items`) avoids conflicts with the `todos`
// table used above. In-memory tests run without Docker; DB tests are ignored.

mod factory_tests {
    autumn_web::reexports::diesel::table! {
        factory_todo_items (id) {
            id -> Int8,
            title -> Text,
            completed -> Bool,
            priority -> Int4,
        }
    }

    #[autumn_web::model(table = "factory_todo_items")]
    pub struct FactoryTodoItem {
        #[id]
        pub id: i64,
        pub title: String,
        pub completed: bool,
        pub priority: i32,
    }

    // ── In-memory tests (no Docker needed) ───────────────────────────────────

    #[test]
    fn factory_build_defaults_all_fields() {
        let t = FactoryTodoItem::factory().build();
        assert_eq!(t.title, "");
        assert!(!t.completed);
        assert_eq!(t.priority, 0);
    }

    #[test]
    fn factory_build_override_title() {
        let t = FactoryTodoItem::factory().title("Buy groceries").build();
        assert_eq!(t.title, "Buy groceries");
        assert_eq!(t.priority, 0); // untouched
    }

    #[test]
    fn factory_build_all_fields() {
        let t = FactoryTodoItem::factory()
            .title("Ship feature")
            .completed(true)
            .priority(1)
            .build();
        assert_eq!(t.title, "Ship feature");
        assert!(t.completed);
        assert_eq!(t.priority, 1);
    }

    #[test]
    fn factory_builds_are_independent() {
        let a = FactoryTodoItem::factory().title("First").build();
        let b = FactoryTodoItem::factory().title("Second").build();
        assert_eq!(a.title, "First");
        assert_eq!(b.title, "Second");
    }

    #[test]
    fn factory_build_returns_insertable_type() {
        let _: NewFactoryTodoItem = FactoryTodoItem::factory().build();
    }

    // ── DB test (requires Docker) ─────────────────────────────────────────────

    #[tokio::test]
    #[ignore = "requires Docker (testcontainers)"]
    async fn factory_create_persists_todo_item() {
        use autumn_web::test::TestDb;

        let db = TestDb::shared().await;
        db.execute_sql(
            "CREATE TABLE IF NOT EXISTS factory_todo_items (
                id        BIGSERIAL PRIMARY KEY,
                title     TEXT NOT NULL DEFAULT '',
                completed BOOL NOT NULL DEFAULT false,
                priority  INT  NOT NULL DEFAULT 0
            )",
        )
        .await;
        db.execute_sql("TRUNCATE factory_todo_items RESTART IDENTITY")
            .await;

        let item = FactoryTodoItem::factory()
            .title("Learn Autumn factories")
            .priority(2)
            .create(&db.pool())
            .await;

        assert!(item.id > 0);
        assert_eq!(item.title, "Learn Autumn factories");
        assert_eq!(item.priority, 2);
        assert!(!item.completed);
    }
}
