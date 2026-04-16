# Macro Transparency

Autumn relies on procedural macros to eliminate boilerplate. This guide shows
you exactly what those macros generate so there are no surprises at runtime.

---

## Startup Log: What Did Autumn Configure?

When your application starts, Autumn logs every decision it makes. A typical
startup sequence looks like this:

```
  INFO autumn: Autumn starting version="0.1.0" profile="dev"
  INFO autumn: Database pool configured max_connections=10
  INFO autumn: Registered task name="db_cleanup" schedule="every 5m"
  INFO autumn: Listening addr=127.0.0.1:3000
```

If you omit the database:

```
  INFO autumn: Autumn starting version="0.1.0" profile="dev"
  INFO autumn: Database not configured
  INFO autumn: Listening addr=127.0.0.1:3000
```

Every line tells you something Autumn decided on your behalf. No silent
configuration -- if the framework did it, it logged it.

### Full transparency mode: `--show-config`

For a complete dump of everything Autumn configured -- every route, every
scheduled task, every middleware layer, and all resolved configuration values
-- use the `--show-config` flag:

```bash
autumn dev --show-config
```

Or with `cargo run`:

```bash
AUTUMN_SHOW_CONFIG=1 cargo run
```

This produces output like:

```
  INFO autumn: Autumn starting version="0.1.0" profile="dev"
  INFO autumn: Registered routes:
    /            GET      -> index
    /todos       GET      -> list_todos
    /todos       POST     -> create_todo
    /todos/{id}  DELETE   -> delete_todo
    /health      GET      -> health
    /actuator/*  GET      -> actuator
  INFO autumn: Scheduled tasks:
    cleanup (every 300s)
  INFO autumn: Active middleware: RequestId, SecurityHeaders, Session (in-memory), CORS, Metrics
  INFO autumn: Configuration:
    profile:    dev
    server:     127.0.0.1:3000
    database:   localhost/mydb (pool_size=10)
    log_level:  debug
    log_format: Pretty
    health:     /health (detailed=true)
    actuator:   sensitive=true
    shutdown:   1s
  INFO autumn: Database pool configured max_connections=10
  INFO autumn: Listening addr=127.0.0.1:3000
```

Database passwords are masked in the output. The log shows the fully resolved
configuration after all 5 layers have been merged, so you can verify that your
env vars, profile overrides, and TOML settings are all taking effect.

### What happens at startup (step by step)

1. **Load configuration** -- 5-layer merge (defaults → profile smart defaults
   → `autumn.toml` → `autumn-{profile}.toml` → `AUTUMN_*` env vars)
2. **Initialize logging** -- format and level come from the merged config
3. **Validate routes** -- panics immediately if no routes are registered
4. **Log banner** -- version and active profile
5. **Create database pool** -- or log "Database not configured" if no URL
6. **Run migrations** -- if `.migrations()` was called and a DB URL exists
7. **Build router** -- mount routes, middleware, static file serving
8. **Start scheduled tasks** -- log each task name and schedule
9. **Bind and listen** -- log the address

---

## Using `cargo expand` to See Generated Code

The most direct way to see what Autumn's macros produce is `cargo expand`.

### Install

```bash
cargo install cargo-expand
```

### Expand a single file

```bash
# Expand your entire crate
cargo expand

# Expand a specific module
cargo expand routes::todos
```

### Tips for readable output

- **Pipe through `rustfmt`** for formatting:
  ```bash
  cargo expand routes::todos | rustfmt
  ```
- **Redirect to a file** to search at your own pace:
  ```bash
  cargo expand > expanded.rs
  ```
- **Search for `__autumn_`** -- all generated companion functions use this
  prefix, making them easy to find in the expanded output.

---

## Macro-by-Macro Expansion Reference

### `#[get("/path")]`, `#[post(...)]`, `#[put(...)]`, `#[delete(...)]`

Your handler function is kept unchanged. The macro adds a hidden companion
function that returns route metadata.

**You write:**

```rust
#[get("/hello")]
async fn hello() -> &'static str {
    "Hello!"
}
```

**The macro generates (alongside your function):**

```rust
pub fn __autumn_route_info_hello() -> ::autumn_web::route::Route {
    ::autumn_web::route::Route {
        method: ::http::Method::GET,
        path: "/hello",
        handler: ::axum::routing::get(hello),
        name: "hello",
    }
}
```

If you add `#[intercept(MyLayer)]`, the handler is wrapped with `.layer()`:

```rust
handler: ::axum::routing::get(hello).layer(MyLayer),
```

### `routes![handler_a, handler_b]`

Transforms a list of handler names into a `Vec<Route>` by calling each
companion function.

**You write:**

```rust
let all = routes![hello, create_todo];
```

**Expands to:**

```rust
let all = vec![
    __autumn_route_info_hello(),
    __autumn_route_info_create_todo(),
];
```

Module-qualified paths work: `routes![users::list, posts::create]` calls
`users::__autumn_route_info_list()` and `posts::__autumn_route_info_create()`.

### `#[autumn_web::main]`

Sets up the Tokio runtime and framework environment variables.

**You write:**

```rust
#[autumn_web::main]
async fn main() {
    autumn_web::app().routes(routes![index]).run().await;
}
```

**Expands to:**

```rust
fn main() {
    autumn_web::config::__set_macro_context(
        env!("CARGO_MANIFEST_DIR").to_string(),
        cfg!(debug_assertions),  // true in debug, false in release
    );

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            autumn_web::app().routes(routes![index]).run().await;
        });
}
```

### `#[model]`

Generates Diesel derives, an insert struct, an update struct with `Patch<T>`
fields, a field enum, and a draft extension trait.

**You write:**

```rust
#[model(table = "posts")]
pub struct Post {
    #[id]
    pub id: i64,
    #[validate(length(min = 1))]
    pub title: String,
    #[default]
    pub published: bool,
}
```

**Generates these types:**

```rust
// 1. Query struct -- your original struct with Diesel derives
#[derive(Queryable, Selectable, AsChangeset, Serialize, Deserialize)]
#[diesel(table_name = posts)]
pub struct Post {
    pub id: i64,
    pub title: String,
    pub published: bool,
}

// 2. Insert struct -- #[id] and #[default] fields excluded
#[derive(Insertable, Serialize, Deserialize)]
#[diesel(table_name = posts)]
pub struct NewPost {
    #[validate(length(min = 1))]
    pub title: String,
}

// 3. Update struct -- all mutable fields wrapped in Patch<T>
#[derive(Serialize, Deserialize, Default)]
pub struct UpdatePost {
    #[serde(default)]
    pub title: Patch<String>,
}

// 4. Field enum
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum PostField {
    Title,
}

// 5. Draft extension trait (for mutation hooks)
pub trait PostDraftExt {
    fn from_patch(current: &Post, patch: &UpdatePost) -> AutumnResult<UpdateDraft<Post>>;
    fn title(&mut self) -> DraftField<'_, String>;
}
```

### `#[repository(Model)]`

Generates a concrete repository struct with CRUD methods, an Axum extractor
impl, and implementations for any derived query methods you declare.

**You write:**

```rust
#[repository(Post)]
pub trait PostRepository {
    fn find_by_published(published: bool) -> Vec<Post>;
    fn count_by_author_id(author_id: i64) -> i64;
}
```

**Generates:**

```rust
// Concrete struct with a connection pool
#[derive(Clone)]
pub struct PgPostRepository {
    pool: deadpool::Pool<AsyncPgConnection>,
}

// Axum extractor -- use `repo: PgPostRepository` in handler signatures
impl FromRequestParts<AppState> for PgPostRepository { ... }

// Built-in CRUD (always generated)
impl PgPostRepository {
    pub async fn find_by_id(&self, id: i64) -> AutumnResult<Post> { ... }
    pub async fn find_all(&self) -> AutumnResult<Vec<Post>> { ... }
    pub async fn save(&self, new: &NewPost) -> AutumnResult<Post> { ... }
    pub async fn update(&self, id: i64, changes: &UpdatePost) -> AutumnResult<Post> { ... }
    pub async fn delete_by_id(&self, id: i64) -> AutumnResult<()> { ... }
    pub async fn count(&self) -> AutumnResult<i64> { ... }
    pub async fn exists_by_id(&self, id: i64) -> AutumnResult<bool> { ... }
}

// Derived queries (parsed from trait method names)
impl PostRepository for PgPostRepository {
    async fn find_by_published(&self, published: bool) -> AutumnResult<Vec<Post>> {
        let mut conn = self.pool.get().await?;
        posts::table
            .filter(posts::published.eq(&published))
            .load::<Post>(&mut conn)
            .await
            .map_err(Into::into)
    }

    async fn count_by_author_id(&self, author_id: i64) -> AutumnResult<i64> {
        let mut conn = self.pool.get().await?;
        posts::table
            .filter(posts::author_id.eq(&author_id))
            .count()
            .get_result(&mut conn)
            .await
            .map_err(Into::into)
    }
}
```

**Derived query name parsing rules:**

| Method prefix   | Generated query                          |
|-----------------|------------------------------------------|
| `find_by_`      | `.filter(col.eq(val)).load()`            |
| `count_by_`     | `.filter(col.eq(val)).count()`           |
| `exists_by_`    | `.filter(col.eq(val)).count() > 0`       |
| `delete_by_`    | `diesel::delete(...).filter(col.eq(val))`|
| `_and_`         | Joins multiple `.filter()` clauses       |

### `#[service]`

Generates a concrete struct and an Axum extractor from a trait with a `deps()`
declaration.

**You write:**

```rust
#[service]
pub trait OrderService {
    fn deps(order_repo: PgOrderRepository, email: EmailClient);
    async fn place_order(&self, req: OrderRequest) -> AutumnResult<Order>;
}
```

**Generates:**

```rust
#[derive(Clone)]
pub struct OrderServiceImpl {
    pub order_repo: PgOrderRepository,
    pub email: EmailClient,
}

// Extractor -- each dependency is extracted from AppState
impl FromRequestParts<AppState> for OrderServiceImpl {
    type Rejection = AutumnError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState)
        -> Result<Self, Self::Rejection>
    {
        Ok(Self {
            order_repo: PgOrderRepository::from_request_parts(parts, state).await?,
            email: EmailClient::from_request_parts(parts, state).await?,
        })
    }
}
```

You implement the business methods on `OrderServiceImpl` yourself.

### `#[scheduled(every = "5m")]`

**You write:**

```rust
#[scheduled(every = "5m", name = "cleanup")]
async fn cleanup(state: AppState) -> AutumnResult<()> {
    // cleanup logic
    Ok(())
}
```

**Generates (alongside your function):**

```rust
pub fn __autumn_task_info_cleanup() -> ::autumn_web::task::TaskInfo {
    ::autumn_web::task::TaskInfo {
        name: "cleanup".to_string(),
        schedule: Schedule::FixedDelay(Duration::from_secs(300)),
        handler: |state| Box::pin(async move { cleanup(state).await }),
    }
}
```

Collected via `tasks![cleanup]` (same pattern as `routes![]`).

### `#[secured("role")]`

Injects a session extractor and an authorization check at the top of your
handler.

**You write:**

```rust
#[get("/admin")]
#[secured("admin")]
async fn admin_panel() -> &'static str {
    "welcome"
}
```

**Effectively becomes:**

```rust
#[get("/admin")]
async fn admin_panel(__autumn_session: Session) -> AutumnResult<&'static str> {
    ::autumn_web::auth::__check_secured(&__autumn_session, &["admin"]).await?;
    Ok("welcome")
}
```

- No roles listed (`#[secured]`) = authentication check only (401 if not
  logged in)
- Roles listed (`#[secured("admin", "editor")]`) = must have at least one of
  the listed roles (403 if missing)

### `#[static_get("/path")]`

Generates both a route companion (same as `#[get]`) and a static metadata
companion for build-time rendering.

**You write:**

```rust
#[static_get("/about")]
async fn about() -> Markup {
    html! { h1 { "About" } }
}
```

**Generates two companions:**

```rust
// Route (same as #[get])
pub fn __autumn_route_info_about() -> Route { ... }

// Static build metadata
pub fn __autumn_static_meta_about() -> StaticRouteMeta {
    StaticRouteMeta {
        path: "/about",
        name: "about",
        revalidate: None,
        params_fn: None,
    }
}
```

---

## The Companion Function Pattern

All Autumn macros follow the same architectural pattern:

1. **Your code stays untouched** (or minimally modified for `#[secured]`)
2. **A hidden `__autumn_*` companion function** is generated next to your code
3. **A collection macro** (`routes![]`, `tasks![]`, `static_routes![]`) calls
   those companions to build typed vectors
4. **The app builder** consumes those vectors at startup

This means:
- Your handler signatures are real Rust -- IDE autocomplete and type checking
  work normally
- The generated code is always next to your code in the expanded output
- There is no runtime reflection, registration, or classpath scanning

---

## Debugging Macro Issues

### "I'm not sure if my macro is being applied"

Search the expanded output for the companion function:

```bash
cargo expand | grep __autumn_route_info_my_handler
```

If it's missing, the attribute macro didn't run. Check that:
- You imported the macro (`use autumn_web::get;` or `use autumn_web::prelude::*;`)
- The attribute is on the function, not on a `mod` block

### "My route isn't being registered"

The macro generates the companion, but `routes![]` must include it:

```rust
// This handler exists but is not mounted:
#[get("/secret")]
async fn secret() -> &'static str { "hidden" }

// Fix: add it to routes![]
.routes(routes![secret])
```

### "cargo expand shows too much noise"

Expand a single module to reduce output:

```bash
cargo expand routes::todos 2>/dev/null | rustfmt
```

### Compiler errors pointing at macro-generated code

The proc macros preserve your original `Span` information, so compiler errors
should point at your source code, not at generated code. If you see an error
in generated code, it usually means:
- A type mismatch in your handler parameters
- A missing `use` import for a Diesel table or type
- A field name in a `find_by_` method that doesn't match a database column
