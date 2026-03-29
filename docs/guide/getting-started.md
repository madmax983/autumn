# Getting Started with Autumn

This guide takes you from zero to a running Autumn web app with routes, a
database, HTML templates, interactive UI, and the current post-`v0.1.0`
framework surface on `trunk`. Budget about 30 minutes.

Autumn is a convention-over-configuration web framework for Rust, built on
[Axum](https://github.com/tokio-rs/axum). It bundles Diesel (database), Maud
(HTML), Tailwind CSS (styling), htmx (interactivity), actuator endpoints,
profile-aware configuration, and newer CLI workflows behind a Spring Boot-style
developer experience.

> **Version note:** `v0.1.0` was tagged on 2026-03-26. This guide tracks the
> current `trunk` branch, which already includes unreleased post-`v0.1.0`
> features like `autumn dev`, `autumn build`, profiles, actuator, and static
> generation.

---

## Prerequisites

- **Rust 1.86.0+** (edition 2024) -- install via [rustup](https://rustup.rs/)
- **PostgreSQL** -- only needed if you want database features; Autumn runs
  fine without one
- A terminal and a text editor

Verify your Rust toolchain:

```bash
rustc --version   # 1.85.0 or later
cargo --version
```

---

## Install the CLI

Autumn ships a small CLI for project scaffolding and tooling setup. Install it
from source (crates.io publication is not yet available):

```bash
cargo install --path autumn-cli
```

This gives you the `autumn` binary with the core workflow commands:

| Command         | What it does                                |
|-----------------|---------------------------------------------|
| `autumn new`    | Scaffold a new project                      |
| `autumn setup`  | Download Tailwind CSS (with checksum verify) |
| `autumn dev`    | Run the dev server with file watching        |
| `autumn build`  | Pre-render `#[static_get]` routes into `dist/` |
| `autumn migrate`| Run migrations or inspect migration status   |

---

## Create a Project

```bash
autumn new my-app
cd my-app
```

This generates:

```
my-app/
  Cargo.toml
  autumn.toml          # framework configuration
  build.rs             # Tailwind CSS build pipeline
  src/
    main.rs            # your application entry point
  static/
    css/
      input.css        # Tailwind directives
  tailwind.config.js
  migrations/          # Diesel migrations (empty for now)
  .gitignore
```

---

## Project Structure

The files that matter right now:

| File                | Purpose                                    |
|---------------------|--------------------------------------------|
| `src/main.rs`       | Routes and application bootstrap           |
| `autumn.toml`       | Server, database, logging, health config   |
| `build.rs`          | Compiles Tailwind CSS on `cargo build`     |
| `static/`           | Auto-served at `/static/` (CSS, JS, images)|
| `migrations/`       | Diesel SQL migrations                      |

---

## Your First Route

Open `src/main.rs`. The scaffolded code looks like this:

```rust
use autumn_web::{get, routes};

#[get("/")]
async fn index() -> &'static str {
    "Welcome to Autumn!"
}

#[get("/hello")]
async fn hello() -> &'static str {
    "Hello, Autumn!"
}

#[get("/hello/{name}")]
async fn hello_name(name: autumn_web::extract::Path<String>) -> String {
    format!("Hello, {}!", *name)
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index, hello, hello_name])
        .run()
        .await;
}
```

The key pieces:

- **`#[get("/path")]`** -- annotates a handler for GET requests. Also
  available: `#[post]`, `#[put]`, `#[delete]`.
- **`routes![...]`** -- collects annotated handlers into a `Vec<Route>`.
- **`autumn_web::app().routes(...).run().await`** -- the app builder. Load config,
  create the database pool, mount routes, start the server.
- **`#[autumn_web::main]`** -- sets up the Tokio async runtime. It is a thin
  wrapper around `#[tokio::main]`.

Handlers are regular async functions. They can return any type that Axum can
turn into an HTTP response: `&str`, `String`, `Json<T>`, `Markup` (Maud HTML),
or your own `impl IntoResponse`.

---

## Run It

```bash
autumn dev
```

If you prefer not to use watch mode:

```bash
cargo run
```

You will see log output like:

```
  INFO autumn: Database not configured
  INFO autumn: Listening addr=127.0.0.1:3000
```

Visit [http://localhost:3000](http://localhost:3000) -- you should see
"Welcome to Autumn!". Try
[http://localhost:3000/hello/world](http://localhost:3000/hello/world) for the
path parameter route.

A health check is automatically mounted at
[http://localhost:3000/health](http://localhost:3000/health). Actuator
endpoints are also auto-mounted at
[http://localhost:3000/actuator/health](http://localhost:3000/actuator/health),
[http://localhost:3000/actuator/info](http://localhost:3000/actuator/info), and
[http://localhost:3000/actuator/metrics](http://localhost:3000/actuator/metrics).

The `/health` response looks like:

```json
{ "status": "ok", "version": "0.1.0" }
```

Press **Ctrl+C** to stop the server (graceful shutdown with a configurable
drain timeout).

---

## Path Parameters

Axum-style path parameters use curly braces in the route pattern and the
`Path<T>` extractor in the handler signature:

```rust
use autumn_web::extract::Path;
use autumn_web::get;

#[get("/users/{id}")]
async fn get_user(id: Path<i32>) -> String {
    format!("User #{}", *id)
}
```

Multiple parameters work by destructuring a tuple:

```rust
#[get("/orgs/{org}/repos/{repo}")]
async fn get_repo(Path((org, repo)): Path<(String, String)>) -> String {
    format!("{org}/{repo}")
}
```

You can also use `Query<T>` for query string parameters:

```rust
use autumn_web::extract::Query;
use serde::Deserialize;

#[derive(Deserialize)]
struct Pagination {
    page: Option<u32>,
    per_page: Option<u32>,
}

#[get("/items")]
async fn list_items(Query(params): Query<Pagination>) -> String {
    let page = params.page.unwrap_or(1);
    let per_page = params.per_page.unwrap_or(20);
    format!("Page {page}, showing {per_page} items")
}
```

---

## Set Up the Database

Autumn uses [Diesel](https://diesel.rs/) with
[diesel-async](https://github.com/weiznich/diesel_async) and
[deadpool](https://docs.rs/deadpool) for async Postgres connections.

### 1. Install the Diesel CLI

```bash
cargo install diesel_cli --no-default-features --features postgres
```

### 2. Create a database

```bash
createdb my_app
```

### 3. Configure the connection

Edit `autumn.toml` and uncomment the `[database]` section:

```toml
[database]
url = "postgres://localhost/my_app"
pool_size = 10
connect_timeout_secs = 5
```

You can also set the URL via environment variable, which takes precedence over
the TOML file:

```bash
export AUTUMN_DATABASE__URL="postgres://localhost/my_app"
```

(Note the double underscore `__` separating section from field.)

### 4. Create a migration

```bash
diesel setup --database-url postgres://localhost/my_app
diesel migration generate create_todos
```

Edit the generated `up.sql`:

```sql
CREATE TABLE todos (
    id SERIAL PRIMARY KEY,
    title TEXT NOT NULL,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);
```

And the `down.sql`:

```sql
DROP TABLE todos;
```

Run it:

```bash
diesel migration run --database-url postgres://localhost/my_app
```

This also generates `src/schema.rs` with Diesel's table macro. If it doesn't
appear, run `diesel print-schema > src/schema.rs`.

---

## Define a Model

Create `src/models.rs`:

```rust
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::todos;

#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = todos)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Todo {
    pub id: i32,
    pub title: String,
    pub completed: bool,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Insertable, Deserialize)]
#[diesel(table_name = todos)]
pub struct NewTodo {
    pub title: String,
}
```

Autumn also provides a `#[model]` attribute macro that auto-derives the
Diesel and Serde traits for you:

```rust
use crate::schema::todos;

// Equivalent to the manual derives above (Queryable, Selectable,
// Insertable, Serialize, Deserialize) plus #[diesel(table_name = todos)]
#[autumn_web::model(table = "todos")]
pub struct Todo {
    pub id: i32,
    pub title: String,
    pub completed: bool,
    pub created_at: chrono::NaiveDateTime,
}
```

If you omit the `table = "..."` argument, the table name is inferred from the
struct name: `BlogPost` becomes `blog_posts`, `User` becomes `users`.

Add the required dependencies to `Cargo.toml`:

```toml
[dependencies]
autumn-web = "0.1.0"
chrono = { version = "0.4", features = ["serde"] }
diesel = { version = "2", features = ["postgres", "chrono"] }
diesel-async = { version = "0.8", features = ["postgres"] }
serde = { version = "1", features = ["derive"] }
```

Don't forget to declare the modules in `main.rs`:

```rust
mod models;
mod schema;
```

---

## Query the Database

Use the `Db` extractor to get an async Postgres connection from the pool.
It implements `Deref<Target = AsyncPgConnection>`, so you can pass `&mut *db`
directly to Diesel queries:

```rust
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{NewTodo, Todo};
use crate::schema::todos;

#[get("/todos")]
async fn list_todos(mut db: Db) -> AutumnResult<Json<Vec<Todo>>> {
    let all_todos = todos::table
        .order(todos::created_at.desc())
        .select(Todo::as_select())
        .load(&mut *db)
        .await?;

    Ok(Json(all_todos))
}

#[post("/api/todos")]
async fn create_todo(mut db: Db, body: Json<NewTodo>) -> AutumnResult<Json<Todo>> {
    let created: Todo = diesel::insert_into(todos::table)
        .values(&body.0)
        .returning(Todo::as_returning())
        .get_result(&mut *db)
        .await?;

    Ok(Json(created))
}
```

Key points:

- **`Db`** is an extractor, not a global. Declare it in your handler signature
  and Autumn hands you a pooled connection. The connection returns to the pool
  when the handler completes.
- **`AutumnResult<T>`** is `Result<T, AutumnError>`. The `?` operator
  converts any `std::error::Error` into an `AutumnError` with status 500.
  Diesel errors, I/O errors, serde errors -- they all "just work" with `?`.
- **`mut db: Db`** -- you need `mut` because Diesel queries take `&mut` on
  the connection.

Register the new handlers in your `main()`:

```rust
mod models;
mod schema;

use autumn_web::routes;

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![list_todos, create_todo])
        .run()
        .await;
}
```

Test it:

```bash
# Create a todo
curl -X POST http://localhost:3000/api/todos \
  -H "Content-Type: application/json" \
  -d '{"title": "Write Autumn guide"}'

# List todos
curl http://localhost:3000/todos
```

---

## Render HTML with Maud

Autumn re-exports [Maud](https://maud.lambda.xyz/), a compile-time HTML
templating library. Return `Markup` from a handler to send HTML:

```rust
use autumn_web::prelude::*;

#[get("/")]
async fn index() -> Markup {
    html! {
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                title { "My App" }
                link rel="stylesheet" href="/static/css/autumn.css";
            }
            body {
                h1 { "Welcome to my app" }
                p { "Built with Autumn." }
            }
        }
    }
}
```

Maud syntax in brief:

| Maud                                  | HTML output                              |
|---------------------------------------|------------------------------------------|
| `h1 { "Hello" }`                     | `<h1>Hello</h1>`                         |
| `div class="box" { "content" }`      | `<div class="box">content</div>`         |
| `input type="text" name="q";`        | `<input type="text" name="q">`           |
| `(variable)`                          | Escaped interpolation                    |
| `(PreEscaped(raw_html))`             | Unescaped interpolation                  |
| `@if cond { ... } @else { ... }`     | Conditional rendering                   |
| `@for item in &items { ... }`        | Loop rendering                           |

Extract reusable layouts into functions:

```rust
fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/css/autumn.css";
                script src="/static/js/htmx.min.js" {}
            }
            body class="bg-gray-100 min-h-screen" {
                div class="max-w-2xl mx-auto py-10 px-4" {
                    (content)
                }
            }
        }
    }
}
```

Then use it in handlers:

```rust
#[get("/about")]
async fn about() -> Markup {
    layout("About", html! {
        h1 { "About this app" }
        p { "Built with Autumn, Maud, and Tailwind." }
    })
}
```

---

## Style with Tailwind CSS

Autumn integrates [Tailwind CSS](https://tailwindcss.com/) via a `build.rs`
that runs the Tailwind standalone CLI at compile time.

### 1. Download Tailwind

```bash
autumn setup
```

This downloads the platform-specific Tailwind CSS binary to
`target/autumn/tailwindcss` with SHA-256 checksum verification. Use
`autumn setup --force` to re-download.

### 2. Write Tailwind classes in your templates

The `build.rs` scans `src/**/*.rs` for class names, so Tailwind utility
classes inside Maud templates are automatically picked up:

```rust
html! {
    div class="max-w-2xl mx-auto py-10 px-4" {
        h1 class="text-3xl font-bold text-gray-800" { "Styled heading" }
        p class="text-gray-500 mt-2" { "A paragraph with Tailwind styles." }
    }
}
```

### 3. Build

```bash
cargo build
```

The build script compiles `static/css/input.css` (which contains Tailwind
directives) into `static/css/autumn.css`. This file is auto-served at
`/static/css/autumn.css`.

Your `input.css` starts with:

```css
@tailwind base;
@tailwind components;
@tailwind utilities;
```

Add custom CSS below the directives as needed.

> **Skipping Tailwind:** If you do not need CSS, delete `build.rs` and the
> Tailwind-related files. Autumn runs fine without them.

---

## Add Interactivity with htmx

Autumn bundles [htmx](https://htmx.org/) and auto-serves it at
`/static/js/htmx.min.js`. Include the script tag in your layout, then use
htmx attributes in your Maud templates.

Here is a toggle button that updates a todo without a full page reload:

```rust
use autumn_web::prelude::*;
use autumn_web::extract::Path;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::Todo;
use crate::schema::todos;

/// Render a single todo item with htmx controls.
fn todo_item(todo: &Todo) -> Markup {
    let title_class = if todo.completed {
        "line-through text-gray-400"
    } else {
        "text-gray-800"
    };

    html! {
        li id=(format!("todo-{}", todo.id))
           class="flex items-center gap-3 p-3 bg-white rounded shadow" {
            // Toggle button -- POST via htmx, swap this <li> with the response
            button hx-post=(format!("/todos/{}/toggle", todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML" {
                @if todo.completed { "\u{2713}" } @else { "\u{25CB}" }
            }
            span class=(title_class) { (todo.title) }
            // Delete button -- returns empty string, htmx removes the element
            button hx-delete=(format!("/todos/{}", todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML" {
                "\u{00D7}"
            }
        }
    }
}

/// Toggle completion status -- returns the updated HTML fragment.
#[post("/todos/{id}/toggle")]
async fn toggle(id: Path<i32>, mut db: Db) -> AutumnResult<Markup> {
    let todo: Todo = todos::table
        .find(*id)
        .select(Todo::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;

    diesel::update(todos::table.find(*id))
        .set(todos::completed.eq(!todo.completed))
        .execute(&mut *db)
        .await?;

    let updated: Todo = todos::table
        .find(*id)
        .select(Todo::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;

    Ok(todo_item(&updated))
}

/// Delete a todo -- returns empty string so htmx removes the element.
#[delete("/todos/{id}")]
async fn delete_todo(id: Path<i32>, mut db: Db) -> AutumnResult<String> {
    diesel::delete(todos::table.find(*id))
        .execute(&mut *db)
        .await?;

    Ok(String::new())
}
```

The htmx attributes you will use most often:

| Attribute     | Purpose                                              |
|---------------|------------------------------------------------------|
| `hx-get`      | Issue a GET request to the URL                       |
| `hx-post`     | Issue a POST request to the URL                      |
| `hx-delete`   | Issue a DELETE request to the URL                    |
| `hx-target`   | CSS selector for the element to update               |
| `hx-swap`     | How to insert the response (`outerHTML`, `innerHTML`, `beforeend`, etc.) |
| `hx-trigger`  | Event that triggers the request (default: natural event) |

The pattern: your handler returns an HTML fragment (not a full page), htmx
swaps it into the DOM. No JavaScript required.

---

## Error Handling

`AutumnResult<T>` is `Result<T, AutumnError>`. Every handler that can fail
should return this type.

### The `?` operator

Any `std::error::Error` converts to `AutumnError` with HTTP 500 automatically:

```rust
#[get("/users")]
async fn list_users(mut db: Db) -> AutumnResult<Json<Vec<User>>> {
    let users = users::table.load(&mut *db).await?; // 500 on failure
    Ok(Json(users))
}
```

### Status refinement

For expected errors, use the status constructors:

```rust
#[get("/users/{id}")]
async fn get_user(id: Path<i32>, mut db: Db) -> AutumnResult<Json<User>> {
    let user = users::table
        .find(*id)
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?; // 404

    Ok(Json(user))
}
```

Available constructors:

| Method                      | HTTP Status                |
|-----------------------------|----------------------------|
| `AutumnError::not_found(e)` | 404 Not Found              |
| `AutumnError::bad_request(e)` | 400 Bad Request          |
| `AutumnError::unprocessable(e)` | 422 Unprocessable Entity |
| `err.with_status(StatusCode::FORBIDDEN)` | Any status code |

Error responses are JSON:

```json
{
  "error": {
    "status": 404,
    "message": "Record not found"
  }
}
```

---

## Configuration

Autumn uses a five-layer configuration system:

1. **Framework defaults** -- compiled into the binary, zero-config start
2. **Profile smart defaults** -- built-in `dev` / `prod` behavior
3. **`autumn.toml`** -- project-level overrides
4. **`autumn-{profile}.toml`** -- profile-specific overrides
5. **`AUTUMN_*` environment variables** -- deployment overrides (highest priority)

### `autumn.toml` reference

```toml
[server]
host = "127.0.0.1"          # default
port = 3000                  # default
shutdown_timeout_secs = 30   # default, seconds to drain in-flight requests

[database]
url = "postgres://user:pass@localhost:5432/my_app"
pool_size = 10               # default, max connections
connect_timeout_secs = 5     # default

[log]
level = "info"               # default, supports tracing filter syntax
format = "Auto"              # Auto | Pretty | Json

[health]
path = "/health"             # default

[actuator]
sensitive = false            # prod default; dev smart defaults enable sensitive endpoints
```

### Environment variable overrides

Every config field can be overridden via environment variables. The pattern is
`AUTUMN_SECTION__FIELD` (double underscore separates section from field):

| Variable                             | Overrides              |
|--------------------------------------|------------------------|
| `AUTUMN_SERVER__PORT`                | `server.port`          |
| `AUTUMN_SERVER__HOST`                | `server.host`          |
| `AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS` | `server.shutdown_timeout_secs` |
| `AUTUMN_DATABASE__URL`               | `database.url`         |
| `AUTUMN_DATABASE__POOL_SIZE`         | `database.pool_size`   |
| `AUTUMN_DATABASE__CONNECT_TIMEOUT_SECS` | `database.connect_timeout_secs` |
| `AUTUMN_LOG__LEVEL`                  | `log.level`            |
| `AUTUMN_LOG__FORMAT`                 | `log.format`           |
| `AUTUMN_HEALTH__PATH`               | `health.path`          |
| `AUTUMN_PROFILE`                     | active profile         |

Profiles resolve in this order:

1. `AUTUMN_PROFILE`
2. `--profile <name>`
3. debug/release auto-detection (`dev` for debug, `prod` for release)

That means you can keep shared defaults in `autumn.toml`, put local dev
settings in `autumn-dev.toml`, and override the final few things in CI or
deployment with env vars.

### Log format behavior

| Format    | Behavior                                              |
|-----------|-------------------------------------------------------|
| `Auto`    | Pretty in development, JSON when `AUTUMN_ENV=production` |
| `Pretty`  | Always human-readable, colorized                     |
| `Json`    | Always structured JSON                                |

### Running without a database

If you omit the `[database]` section (or leave `url` unset), Autumn starts
without a database pool. Handlers that use `Db` will return 503 Service
Unavailable. This is useful for static sites, APIs that do not need a
database, or during early development.

---

## What's Next?

You now have a working Autumn application with routes, database access,
HTML rendering, Tailwind styling, htmx interactivity, health checks, and
actuator endpoints.

Here are some things to explore:

- **Organize routes into modules** -- call `.routes()` multiple times on the
  app builder to compose route groups:
  ```rust
  autumn_web::app()
      .routes(routes![index])
      .routes(routes![list_todos, create_todo, toggle, delete_todo])
      .run()
      .await;
  ```
- **Use `Form<T>`** for HTML form submissions instead of JSON:
  ```rust
  use autumn_web::extract::Form;

  #[post("/todos")]
  async fn create(mut db: Db, form: Form<NewTodo>) -> AutumnResult<Markup> {
      // form.0 is the deserialized NewTodo
      // ...
  }
  ```
- **Check `/health` and `/actuator/*`** -- `/health` gives a small health
  response, while actuator adds info, metrics, env/configprops, loggers, and
  scheduled task visibility depending on the active profile.
- **Inspect request IDs** -- every response includes an `X-Request-Id` header
  (UUID v4) for log correlation.
- **Look at the example apps** -- [`examples/todo-app`](../../examples/todo-app),
  [`examples/blog`](../../examples/blog), [`examples/bookmarks`](../../examples/bookmarks),
  and [`examples/wiki`](../../examples/wiki) each exercise different parts of
  the current framework.

Autumn is still pre-1.0 and evolving quickly. File issues, break glass when you
need Axum escape hatches, and ship something.
