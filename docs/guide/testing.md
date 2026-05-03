# Integration Testing

Autumn ships a first-party test surface (`autumn_web::test`) that brings
Rails-grade ergonomics to Rust integration testing: one line to boot a fully-wired
app, assertions that chain, and a shared Postgres testcontainer that keeps your
test suite fast.

> **Reference implementation:** `examples/blog/tests/integration_test.rs`
> exercises every concept on this page against real blog routes.

---

## The public surface

| Type | Purpose | Spring Boot analogy |
|------|---------|---------------------|
| [`TestApp`] | Boot a fully-wired Autumn app in-process | `@SpringBootTest` |
| [`TestClient`] | Fluent HTTP request builder | `MockMvc` / `WebTestClient` |
| [`TestResponse`] | Response with chainable assertion helpers | `MvcResult` |
| [`TestDb`] | Shared Postgres testcontainer | `@DataJpaTest` |

[`TestApp`]: https://docs.rs/autumn-web/latest/autumn_web/test/struct.TestApp.html
[`TestClient`]: https://docs.rs/autumn-web/latest/autumn_web/test/struct.TestClient.html
[`TestResponse`]: https://docs.rs/autumn-web/latest/autumn_web/test/struct.TestResponse.html
[`TestDb`]: https://docs.rs/autumn-web/latest/autumn_web/test/struct.TestDb.html

`TestApp` fires requests through the full Axum middleware pipeline using
`tower::ServiceExt::oneshot()` — the same security, tracing, rate-limiting,
and routing stack you run in production, minus the TCP listener.

---

## Quick start — no Docker required

Add `autumn_web::test` to your integration test and write your first assertion:

```rust
// tests/integration_test.rs
use autumn_web::prelude::*;
use autumn_web::test::TestApp;

#[get("/hello")]
async fn hello() -> &'static str { "Hello, Autumn!" }

#[tokio::test]
async fn hello_returns_200() {
    let client = TestApp::new()
        .routes(routes![hello])
        .build();

    client.get("/hello").send().await
        .assert_ok()
        .assert_body_contains("Autumn");
}
```

Run it:

```bash
cargo test
```

No Docker, no extra setup. `TestApp::new()` uses the `"test"` profile by
default and disables CSRF so form submissions work without a session token.

---

## Autumn-specific assertions

Every Autumn response carries `X-Request-Id` (set by `RequestIdLayer`). You
can assert on it as a framework-level signal that the full middleware stack ran:

```rust
#[tokio::test]
async fn autumn_attaches_request_id_to_every_response() {
    let client = TestApp::new().routes(routes![hello]).build();
    let resp = client.get("/hello").send().await;

    assert!(
        resp.header("x-request-id").is_some(),
        "Autumn's RequestIdLayer must attach X-Request-Id to every response"
    );
}
```

Other useful assertions on `TestResponse`:

```rust
resp
    .assert_ok()                                   // 200 OK
    .assert_status(201)                            // specific status
    .assert_success()                              // any 2xx
    .assert_header("content-type", "text/plain")   // exact header value
    .assert_header_contains("content-type", "json") // substring
    .assert_body_contains("Alice")                 // body substring
    .assert_body_eq("pong")                        // exact body
    .assert_body_empty()                           // empty body
    .assert_json::<MyType, _>(|val| {              // deserialize + check
        assert_eq!(val.name, "Alice");
    });
```

---

## Database integration tests

For tests that need a real database, `TestDb` wraps a Postgres testcontainer.
The container starts once per test binary and is shared across all tests —
no one-container-per-test overhead.

### 1  Add the dev-dependency

```toml
# Cargo.toml  — use the same version as your [dependencies] entry
[dev-dependencies]
autumn-web = { version = "0.3", features = ["test-support"] }
serde_json = "1"
```

Replace `"0.3"` with whatever version you have in `[dependencies]` (or omit
`version` entirely and rely on Cargo's workspace resolution).

The `test-support` feature activates `TestDb`. No other dev-dependency is
needed; `diesel`, `diesel-async`, `serde`, and `tokio` are already in your
`[dependencies]`.

### 2  Define your schema and handlers inline

Integration tests in `tests/` are separate crates that cannot import from
`src/main.rs` (binary crates don't expose a library target). Define the schema
and handler under test inline — or extract them into a `src/lib.rs` for larger
apps:

```rust
// tests/integration_test.rs
use autumn_web::prelude::*;
use autumn_web::test::{TestApp, TestDb};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};

diesel::table! {
    posts (id) {
        id -> Int8,
        title -> Text,
        slug -> Text,
        body -> Text,
        published -> Bool,
    }
}

#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = posts)]
struct Post { id: i64, title: String, slug: String, body: String, published: bool }

#[derive(Insertable, Deserialize)]
#[diesel(table_name = posts)]
struct NewPost { title: String, slug: String, body: String, #[serde(default)] published: bool }

#[get("/api/posts")]
async fn list_published(mut db: Db) -> AutumnResult<Json<Vec<Post>>> {
    let rows = posts::table
        .filter(posts::published.eq(true))
        .select(Post::as_select())
        .load(&mut *db).await?;
    Ok(Json(rows))
}

#[post("/api/posts")]
async fn create_post(mut db: Db, Json(body): Json<NewPost>) -> AutumnResult<Json<Post>> {
    let created = diesel::insert_into(posts::table)
        .values(&body)
        .returning(Post::as_returning())
        .get_result(&mut *db).await?;
    Ok(Json(created))
}
```

### 3  Spin up the container and run your test

```rust
async fn setup() -> diesel_async::pooled_connection::deadpool::Pool<
    diesel_async::AsyncPgConnection
> {
    let db = TestDb::shared().await;           // shared container — starts once
    db.execute_sql(
        "CREATE TABLE IF NOT EXISTS posts (
            id BIGSERIAL PRIMARY KEY,
            title TEXT NOT NULL,
            slug  TEXT NOT NULL DEFAULT '',
            body  TEXT NOT NULL DEFAULT '',
            published BOOLEAN NOT NULL DEFAULT false
        )",
    ).await;
    db.execute_sql("TRUNCATE posts RESTART IDENTITY").await;
    db.pool()
}

/// DB round-trip: create a post, then verify it appears in the listing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn create_post_round_trip() {
    let pool = setup().await;
    let client = TestApp::new()
        .routes(routes![list_published, create_post])
        .with_db(pool)
        .build();

    // Initially empty
    client.get("/api/posts").send().await.assert_ok().assert_body_eq("[]");

    // DB write
    client
        .post("/api/posts")
        .json(&serde_json::json!({
            "title": "Hello from Autumn tests",
            "slug":  "hello-autumn-tests",
            "body":  "Created in an integration test.",
            "published": true
        }))
        .send().await
        .assert_ok()
        .assert_header_contains("content-type", "application/json")
        .assert_json::<serde_json::Value, _>(|post| {
            assert_eq!(post["title"], "Hello from Autumn tests");
            assert!(post["id"].as_i64().unwrap() > 0);
        });

    // DB read — confirm the write persisted
    client.get("/api/posts").send().await
        .assert_ok()
        .assert_json::<Vec<serde_json::Value>, _>(|posts| {
            assert_eq!(posts.len(), 1);
            assert_eq!(posts[0]["title"], "Hello from Autumn tests");
        });
}
```

### 4  Run the tests

```bash
# smoke tests only (instant, no Docker)
cargo test

# include Docker-backed DB tests
cargo test -- --include-ignored

# or opt-in via an env var in CI
cargo test -- --include-ignored   # set DOCKER_HOST or TESTCONTAINERS_HOST
```

---

## Why `#[ignore = "requires Docker"]`?

Marking DB tests as `#[ignore]` means `cargo test` (no flags) runs green
everywhere — CI machines without Docker, laptops without a running daemon,
etc. Developers who have Docker available opt in with `--include-ignored`.

This mirrors how Autumn's own test suite handles `test_db_integration.rs`.

---

## Running doctests

The `autumn_web::test` module itself ships runnable doctests. Run them with:

```bash
cargo test --doc -p autumn-web
```

---

## Test-data factories

Every `#[model]` type automatically gets a `{Model}Factory` builder. Tests
declare only the fields that matter — everything else stays at its type default
(`""` for `String`, `0` for integers, `false` for `bool`, `None` for
`Option<T>`).

### Declaring a model

```rust
// src/models.rs
use crate::schema::posts;

#[autumn_web::model]
pub struct Post {
    #[id]
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub body: String,
    pub published: bool,
    pub views: i32,
}
```

The macro generates `Post::factory()` → `PostFactory`, along with setter
methods for each non-ID field and two terminus methods:

| Method | Returns | Description |
|--------|---------|-------------|
| `.build()` | `NewPost` | In-memory struct, no database |
| `.create(&pool).await` | `Post` | Insert + return with PK populated |

### Building in-memory instances

```rust
// Zero required args — only override what your test cares about
let draft: NewPost = Post::factory().build();
assert_eq!(draft.title, "");

// Override one field; all others stay at default
let draft = Post::factory().title("Hello TDD").build();
assert_eq!(draft.title, "Hello TDD");
assert_eq!(draft.views, 0);            // untouched

// Override several fields
let draft = Post::factory()
    .title("Published piece")
    .slug("published-piece")
    .published(true)
    .build();
assert!(draft.published);
assert_eq!(draft.body, "");            // untouched
```

### Persisting to the database

Use `.create(&pool)` when a test needs a real DB row:

```rust
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn post_appears_in_listing() {
    let db = TestDb::shared().await;
    // (run CREATE TABLE ... first)

    let post = Post::factory()
        .title("TDD post")
        .published(true)
        .create(&db.pool())
        .await;

    assert!(post.id > 0);
    assert_eq!(post.title, "TDD post");
}
```

### Factory composition

When one model references another (e.g. a `Comment` that belongs to a `Post`),
build the parent first and pass its primary key:

```rust
// 1. Persist the parent
let post = Post::factory().title("Parent").create(&db.pool()).await;

// 2. Build the child referencing the parent's id
let comment = Comment::factory()
    .post_id(post.id)
    .body("Great read!")
    .create(&db.pool())
    .await;

assert_eq!(comment.post_id, post.id);
```

This keeps associations explicit and avoids hidden global state or
infinite-recursion footguns.

### Line-count benchmark

The success metric from the original spec: a "create user with one published
post and one comment" fixture should be **≤ 8 lines** of intent, down from
**≥ 25 lines** of struct-literal boilerplate:

```rust
// Before: 25+ lines of NewUser { id: 1, email: "a@b.c".into(), ... }
// After factory pattern:
let user    = User::factory().email("alice@example.com").create(&pool).await;
let post    = Post::factory().user_id(user.id).title("Hello").published(true).create(&pool).await;
let comment = Comment::factory().post_id(post.id).body("Nice!").create(&pool).await;
```

---

## Patterns at a glance

| Scenario | Pattern |
|----------|---------|
| No-DB smoke test | `TestApp::new().routes(routes![...]).build()` |
| Custom config | `.config(AutumnConfig { … })` or `.profile("staging")` |
| With database | `.with_db(TestDb::shared().await.pool())` |
| Authorization | `.policy(MyPolicy).scope(MyScope)` |
| Custom middleware | `.layer(MyLayer)` |
| Raw router | `TestApp::from_router(my_router)` |
| Build model in-memory | `MyModel::factory().field(val).build()` |
| Persist model to DB | `MyModel::factory().field(val).create(&pool).await` |

---

*Next: [Tutorial Chapter 11 — Writing Tests](tutorial/11-testing.md)*
