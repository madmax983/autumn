---
name: autumn-web
description: >
  How to build web applications with autumn-web, a Spring Boot-style Rust web framework
  built on Axum. Use this skill whenever the user mentions autumn-web, autumn web framework,
  or wants to build a Rust web app using autumn conventions. Also trigger when you see
  references to autumn-web's macros (#[get], #[post], #[secured], #[scheduled], #[model],
  #[repository]), its AppBuilder API, Maud templates with htmx, or Diesel async Postgres
  integration through autumn. If the user mentions "autumn" in a Rust web context, this is
  almost certainly the framework they mean — use this skill proactively.
---

# autumn-web — Rust Web Framework

**Repository**: https://github.com/madmax983/autumn  
**Branch**: `trunk-dev`  
**Version**: 0.2.0 | **Edition**: 2024 | **MSRV**: 1.88.0  
**Author**: madmax983  

autumn-web is a Spring Boot-style web framework for Rust, built on Axum. It assembles
proven crates (Axum, Diesel, Maud, htmx, Tailwind) into a convention-over-configuration
stack with proc-macro ergonomics.

## When to read reference files

This SKILL.md covers everything you need to scaffold and build an autumn-web project.
For deeper details, read these files from the `references/` directory next to this file:

- **`references/api-reference.md`** — Full lib.rs re-exports, prelude.rs contents,
  Cargo.toml features, workspace dependency versions. Read this when you need exact
  type names, feature flags, or dependency versions.
- **`references/examples.md`** — Complete main.rs and Cargo.toml from the blog,
  todo-app, and reddit-clone examples. Read this when building a full app to see
  idiomatic patterns in context. The **reddit-clone** is the most comprehensive example
  and uses every framework feature including autumn-harvest workflows.

---

## Project Structure

```
my-app/
├── src/
│   ├── main.rs              # AppBuilder setup, migrations, route/task registration
│   ├── models.rs            # Diesel Queryable/Insertable structs (or #[model] macro)
│   ├── schema.rs            # Diesel table! definitions
│   ├── routes/
│   │   ├── mod.rs           # Re-exports
│   │   ├── auth.rs          # Login/register/logout
│   │   ├── posts.rs         # CRUD routes
│   │   └── api.rs           # JSON API
│   ├── templates/           # Maud template functions (optional organization)
│   │   ├── mod.rs
│   │   └── layout.rs
│   ├── tasks.rs             # #[scheduled] background tasks
│   └── workflows/           # autumn-harvest workflows (if using harvest)
├── migrations/
│   └── 00000000000000_init/
│       ├── up.sql
│       └── down.sql
├── static/                  # Static assets (CSS, JS, images)
├── Cargo.toml
├── autumn.toml              # Framework config
└── autumn-dev.toml          # Dev profile overrides
```

## Cargo.toml

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2024"

[dependencies]
autumn-web = { version = "0.2", features = ["ws"] }
chrono = { version = "0.4", features = ["serde"] }
diesel = { version = "2", features = ["postgres", "chrono"] }
diesel-async = { version = "0.8", features = ["postgres"] }
pq-sys = { version = "0.7", features = ["bundled_without_openssl"] }
diesel_migrations = "2"
maud = { version = "0.27", features = ["axum"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
validator = { version = "0.20", features = ["derive"] }
uuid = { version = "1", features = ["v4"] }
tracing = "0.1"
```

**Feature flags** (default: `maud`, `htmx`, `tailwind`, `db`, `cache-moka`):

| Feature | Purpose |
|---------|---------|
| `ws` | WebSocket support (`#[ws]` macro) |
| `flash` | Flash messages |
| `multipart` | Multipart form uploads |
| `redis` | Redis-backed sessions |
| `oauth2` | OAuth2/OIDC |
| `openapi` | OpenAPI spec generation |
| `telemetry-otlp` | OpenTelemetry export |
| `test-support` | Testcontainers integration |

## main.rs — The Entry Point

Every autumn app follows this pattern:

```rust
mod models;
mod routes;
mod schema;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::posts::list,
            routes::posts::create,
            routes::auth::login,
            routes::auth::logout,
        ])
        .tasks(tasks![
            tasks::recalculate_counts,
        ])
        .run()
        .await;
}
```

### AppBuilder API

| Method | Purpose |
|--------|---------|
| `.routes(routes![...])` | Register route handlers |
| `.static_routes(static_routes![...])` | Pre-rendered routes (`#[static_get]`) |
| `.tasks(tasks![...])` | Scheduled background tasks |
| `.migrations(MIGRATIONS)` | Embedded Diesel migrations |
| `.plugins(plugin)` | Framework plugins (e.g. `HarvestPlugin`) |
| `.scoped(scope)` | Scoped sub-application |
| `.merge(router)` | Merge an Axum router |
| `.nest(path, router)` | Nest a sub-router at a path |
| `.error_pages(renderer)` | Custom error page rendering |
| `.layer(layer)` | Add Tower middleware layer |
| `.run()` | Launch the server |

## Route Macros

```rust
#[get("/posts")]
async fn list(db: Db) -> AutumnResult<Markup> { ... }

#[get("/posts/{id}")]
async fn show(Path(id): Path<i64>, db: Db) -> AutumnResult<Markup> { ... }

#[post("/posts")]
#[secured]
async fn create(db: Db, Valid(Form(input)): Valid<Form<CreatePost>>) -> AutumnResult<Markup> { ... }

#[put("/posts/{id}")]
async fn update(Path(id): Path<i64>, db: Db) -> AutumnResult<Markup> { ... }

#[delete("/posts/{id}")]
async fn delete_post(Path(id): Path<i64>, db: Db) -> AutumnResult<Markup> { ... }

#[static_get("/about")]
async fn about() -> Markup { html! { h1 { "About" } } }

#[ws("/socket")]
async fn ws() -> impl WsHandler {
    |mut socket: WebSocket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            if let Message::Text(text) = msg { socket.send(Message::Text(text)).await.ok(); }
        }
    }
}

#[scheduled(every = "15m")]
async fn background_job(db: Db) -> AutumnResult<()> { Ok(()) }
```

Route functions are collected via `routes![handler1, handler2]` — this macro calls each
handler's generated companion function to produce a `Vec<Route>`.

## Extractors (from `autumn_web::prelude::*`)

| Extractor | Source | Notes |
|-----------|--------|-------|
| `Db` | Connection pool | Async Diesel Postgres connection |
| `Session` | Cookie | `.get("key")`, `.set("key", val)`, `.clear()`, `.rotate_id()` |
| `Auth` | Session | `.user_id()` — requires `#[secured]` |
| `CsrfToken` | Middleware | `.token()` for form hidden field |
| `Path(val)` | URL path | `Path(id): Path<i64>` |
| `Query(val)` | Query string | `Query(params): Query<Params>` |
| `Form(val)` | Form body | `Form(input): Form<Input>` |
| `Json(val)` | JSON body | `Json(data): Json<Data>` |
| `Valid<T>` | Wraps above | Auto-validates via `validator::Validate`, returns 422 on failure |
| `PageRequest` | Query string | `{ page, size }` — use with `Page::new()` for pagination |
| `Flash` | Cookie (flash feature) | Flash messages between redirects |
| `HxRequest` | htmx header | Detect htmx requests |
| `Multipart` | multipart feature | File uploads |
| `State(state)` | App state | `State(state): State<AppState>` |

## Authentication & Security

```rust
// Login: verify password, set session
#[post("/login")]
async fn login(db: Db, Form(input): Form<LoginInput>, mut session: Session) -> AutumnResult<Markup> {
    let user = find_user_by_email(&mut *db, &input.email).await?;
    if bcrypt::verify(&input.password, &user.password_hash).is_ok() {
        session.set("user_id", &user.id).await?;
        session.set("user_role", &user.role).await?;  // optional role
        session.rotate_id().await?;
        // redirect or render success
    }
    // ...
}

// Protected route — any authenticated user
#[get("/dashboard")]
#[secured]
async fn dashboard(session: Session) -> AutumnResult<Markup> { ... }

// Role-restricted route
#[get("/admin")]
#[secured("admin")]
async fn admin_panel() -> AutumnResult<Markup> { ... }

// CSRF in forms
#[get("/form")]
async fn form(csrf: CsrfToken) -> Markup {
    html! {
        form method="POST" action="/submit" {
            input type="hidden" name="_csrf" value=(csrf.token());
            // ... fields
            button { "Submit" }
        }
    }
}
```

## Models & Database

autumn-web uses Diesel + diesel-async for Postgres. Models can use the `#[model]` macro
or standard Diesel derives:

```rust
// With #[model] macro (generates Diesel derives)
#[model(table = "posts")]
#[derive(Validate)]
pub struct Post {
    pub id: i64,
    #[validate(length(min = 1, max = 500))]
    pub title: String,
    pub body: String,
    pub user_id: i64,
    pub created_at: DateTime<Utc>,
}

// Standard Diesel approach (also works)
#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = crate::schema::posts)]
pub struct Post { ... }

#[derive(Insertable, Deserialize, Validate)]
#[diesel(table_name = crate::schema::posts)]
pub struct NewPost { ... }
```

### Repository pattern

```rust
// Using #[repository] macro (generates CRUD + REST API)
#[repository]
pub struct PostRepository { db: Arc<PgPool> }

// Or manual repository functions (common pattern)
pub async fn find_post_by_id(conn: &mut AsyncPgConnection, post_id: i64) -> AutumnResult<Post> {
    use crate::schema::posts::dsl::*;
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;

    posts.filter(id.eq(post_id))
        .first(conn)
        .await
        .map_err(|_| AutumnError::not_found_msg("Post not found"))
}
```

### Migrations

Place in `migrations/TIMESTAMP_name/up.sql` and `down.sql`:

```sql
CREATE TABLE posts (
    id BIGSERIAL PRIMARY KEY,
    title VARCHAR(500) NOT NULL,
    body TEXT NOT NULL,
    user_id BIGINT NOT NULL REFERENCES users(id),
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);
CREATE INDEX posts_user_id_idx ON posts(user_id);
```

Embed in main.rs with `embed_migrations!()` and register with `.migrations(MIGRATIONS)`.

## Maud Templates + htmx

Maud is a compile-time HTML macro. htmx is bundled and served at `/static/js/htmx.min.js`.

```rust
use autumn_web::prelude::*;

pub fn page(title: &str, content: Markup) -> Markup {
    html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                title { (title) }
                script src=(HTMX_JS_PATH) {}
                script src=(HTMX_CSRF_JS_PATH) {}
                // Tailwind via CDN or autumn build
            }
            body class="bg-gray-950 text-white" {
                nav { /* navigation */ }
                main { (content) }
            }
        }
    }
}

// htmx infinite scroll example
#[get("/feed")]
async fn feed(db: Db) -> AutumnResult<Markup> {
    Ok(page("Feed", html! {
        div id="feed" hx-get="/feed/posts?page=1" hx-trigger="load" hx-swap="innerHTML" {
            // loaded via htmx
        }
    }))
}

#[get("/feed/posts")]
async fn feed_partial(db: Db, PageRequest { page, size }: PageRequest) -> AutumnResult<Markup> {
    let posts = fetch_posts(&mut *db, page, size).await?;
    Ok(html! {
        @for post in &posts {
            article { h2 { (post.title) } p { (post.body) } }
        }
        @if posts.len() == size as usize {
            div hx-get=(format!("/feed/posts?page={}&size={}", page + 1, size))
                hx-trigger="revealed" hx-swap="outerHTML" { "Loading..." }
        }
    })
}
```

## Pagination

```rust
async fn list(db: Db, PageRequest { page, size }: PageRequest) -> AutumnResult<Json<Page<Post>>> {
    let total = count_posts(&mut *db).await?;
    let items = fetch_posts_paginated(&mut *db, page, size).await?;
    Ok(Json(Page::new(items, page, size, total)))
}
// Query: GET /posts?page=1&size=20
```

## Configuration

**autumn.toml** (base config):
```toml
[server]
port = 3000
host = "0.0.0.0"

[database]
url = "postgres://localhost:5432/myapp"

[session]
store = "memory"        # "redis" in production
cookie_name = "session"
max_age_secs = 86400

[security]
csrf_enabled = true

[logging]
level = "info"
format = "pretty"       # "json" in production
```

**autumn-dev.toml** (overrides when `AUTUMN_PROFILE=dev`):
```toml
[database]
url = "postgres://localhost:5432/myapp_dev"

[logging]
level = "debug"
```

**Env var overrides**: `AUTUMN_SERVER_PORT=8080`, `AUTUMN_DATABASE_URL="postgres://..."`, etc.

**Priority**: `autumn-{profile}.toml` > `autumn.toml` > `AUTUMN_*` env vars > defaults.

## Scheduled Tasks

```rust
#[scheduled(every = "1h")]
pub async fn expire_stale_records(db: Db) -> AutumnResult<()> {
    // runs every hour
    Ok(())
}

// Register in main.rs:
.tasks(tasks![expire_stale_records])
```

Visible at `/actuator/tasks` endpoint.

## Auto-Mounted Endpoints

Autumn automatically serves:

| Endpoint | Purpose |
|----------|---------|
| `GET /health` | Simple health check |
| `GET /actuator/health` | Detailed health status |
| `GET /actuator/info` | App version, uptime |
| `GET /actuator/tasks` | Scheduled task status |
| `GET /live` | K8s liveness probe |
| `GET /ready` | K8s readiness probe |
| `GET /startup` | K8s startup probe |
| `GET /static/js/htmx.min.js` | Bundled htmx |

## Plugins

Plugins extend the `AppBuilder`. The main built-in plugin is `HarvestPlugin` for
durable workflows (see the **autumn-harvest** skill for details).

```rust
.plugins(harvest_runtime::harvest_plugin())
// or multiple:
.plugins((plugin_a(), plugin_b()))
```

## CLI

```bash
autumn new my-app      # Scaffold a new project
autumn setup           # Download Tailwind CSS
autumn dev             # Hot-reload dev server
autumn build           # Pre-render #[static_get] routes to dist/
autumn migrate         # Run database migrations
cargo run              # Run without CLI
```

## Error Handling

```rust
async fn handler() -> AutumnResult<String> {
    // Common error constructors:
    Err(AutumnError::not_found_msg("not found"))?;
    Err(AutumnError::bad_request_msg("invalid input"))?;
    Err(AutumnError::unauthorized())?;
    Err(AutumnError::internal_server_error())?;
    Ok("ok".into())
}
```

## Validation

```rust
#[derive(Deserialize, Validate)]
pub struct CreatePost {
    #[validate(length(min = 1, max = 500))]
    pub title: String,
    #[validate(email)]
    pub email: String,
}

// Auto-validated — returns 422 on failure:
async fn create(Valid(Form(input)): Valid<Form<CreatePost>>) -> AutumnResult<String> {
    // input is guaranteed valid here
    Ok(format!("Title: {}", input.title))
}
```

## Custom Error Pages

```rust
use autumn_web::error_pages::{ErrorPageRenderer, ErrorContext};

struct MyErrorPages;
impl ErrorPageRenderer for MyErrorPages {
    fn render_error(&self, ctx: &ErrorContext) -> Markup {
        html! { h1 { (ctx.status) } p { (ctx.message) } }
    }
}

// In main.rs:
.error_pages(MyErrorPages)
```

## Key Dependencies & Versions

| Crate | Version | Purpose |
|-------|---------|---------|
| `axum` | 0.8 | HTTP routing |
| `diesel` | 2 | ORM |
| `diesel-async` | 0.8 | Async Diesel |
| `maud` | 0.27 | HTML templates |
| `tokio` | 1 | Async runtime |
| `validator` | 0.20 | Input validation |
| `bcrypt` | 0.19 | Password hashing |
| `tower-http` | 0.6 | HTTP middleware |
| `tracing` | 0.1 | Structured logging |

## Tips

- Path parameters use `{name}` syntax: `#[get("/users/{id}")]`
- `HTMX_JS_PATH` and `HTMX_CSRF_JS_PATH` constants give correct script paths
- Use `pq-sys = { version = "0.7", features = ["bundled_without_openssl"] }` to
  avoid needing system libpq
- The reddit-clone example is the most comprehensive reference for a full-featured app
- Use `[patch.crates-io]` in workspace Cargo.toml to unify autumn-web across workspace:
  `autumn-web = { path = "autumn" }`
