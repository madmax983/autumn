# Autumn: A Spring Boot-Inspired Application Framework for Rust

**Status:** Draft / Early Design
**Author:** Mark
**Date:** March 2026

---

## Vision

Rust has all the pieces for building production web applications — Axum, Diesel, Tower, Serde, tracing — but nobody has assembled them into an opinionated, convention-over-configuration framework that makes the first five minutes feel effortless. Autumn is that framework.

Autumn is not a port of Spring Boot. It's the answer to the same question Spring Boot answered for Java: *what if starting a new business application was boring?* You declare what you want — endpoints, models, config — and Autumn wires the plumbing. When you outgrow the defaults, you write Rust directly and the framework steps aside.

Autumn is application-first, not API-first. The default output is a server-rendered HTML page, not a JSON response. Your whole app — routes, templates, database, assets — ships as a single Rust binary.

The stack: Axum for HTTP. Maud for compile-time HTML templating. Tailwind CSS for styling. htmx for interactivity without a JS build step. Diesel for ORM. Postgres for storage. TOML for config. Proc macros for the developer experience. Everything else is just smart defaults.

---

## Hello World

### The Minimal Version

```rust
use autumn::prelude::*;

#[autumn::main]
async fn main() {
    autumn::run().await;
}

#[get("/")]
async fn index() -> Markup {
    html! {
        h1 { "Hello, Autumn." }
    }
}
```

That's it. `autumn::run()` reads config, binds the server, discovers annotated routes, sets up tracing, serves static assets, and starts listening. Zero boilerplate. The response is HTML by default.

### The Realistic Version

```rust
use autumn::prelude::*;

#[derive(Model, Serialize, Deserialize)]
#[table_name = "users"]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: String,
}

// HTML is the default. Return Markup, get HTML.
#[get("/users")]
async fn list_users(db: Db) -> Markup {
    let users = users::table.load::<User>(&db).await?;
    html! {
        div class="max-w-2xl mx-auto p-6" {
            h1 class="text-3xl font-bold text-gray-900" { "Users" }
            ul class="mt-4 space-y-2" {
                @for user in &users {
                    li class="p-3 bg-white rounded-lg shadow" {
                        a href={"/users/" (user.id)}
                          class="text-blue-600 hover:text-blue-800" {
                            (user.name)
                        }
                    }
                }
            }
            form hx-post="/users" hx-target="ul" hx-swap="beforeend"
                 class="mt-6 flex gap-2" {
                input type="text" name="name" placeholder="Name"
                      class="border rounded px-3 py-2";
                input type="email" name="email" placeholder="Email"
                      class="border rounded px-3 py-2";
                button type="submit"
                       class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700" {
                    "Add User"
                }
            }
        }
    }
}

#[get("/users/{id}")]
async fn get_user(id: Path<i32>, db: Db) -> Markup {
    let user = users::table.find(*id).first::<User>(&db).await?;
    html! {
        div class="max-w-2xl mx-auto p-6" {
            h1 class="text-3xl font-bold" { (user.name) }
            p class="text-gray-600 mt-2" { (user.email) }
        }
    }
}

#[post("/users")]
async fn create_user(body: Form<NewUser>, db: Db) -> Markup {
    let user = diesel::insert_into(users::table)
        .values(&*body)
        .get_result::<User>(&db)
        .await?;
    // htmx swap target — just return the new <li>
    html! {
        li class="p-3 bg-white rounded-lg shadow" {
            a href={"/users/" (user.id)}
              class="text-blue-600 hover:text-blue-800" {
                (user.name)
            }
        }
    }
}

// JSON is the escape hatch. Return Json<T>, get JSON.
// Same model, same DB, different serialization.
#[get("/api/users")]
async fn list_users_api(db: Db) -> Json<Vec<User>> {
    let users = users::table.load::<User>(&db).await?;
    Json(users)
}

#[get("/api/users/{id}")]
async fn get_user_api(id: Path<i32>, db: Db) -> Json<User> {
    let user = users::table.find(*id).first::<User>(&db).await?;
    Json(user)
}

#[autumn::main]
async fn main() {
    autumn::run().await;
}
```

With an `autumn.toml`:

```toml
[server]
port = 3000

[database]
url = "postgres://localhost/myapp"

[logging]
level = "info"
```

You now have a complete web application — server-rendered HTML with interactive forms, plus a JSON API for external consumers. Connection pooling, structured logging, error handling, htmx served automatically, and health checks at `/health`. You wrote zero infrastructure code. The return type is the contract: `Markup` means HTML, `Json<T>` means JSON.

---

## Architecture

### Crate Stack

| Layer | Crate | Role |
|-------|-------|------|
| HTTP | Axum | Router, extractors, middleware |
| Templating | Maud | Compile-time HTML generation |
| Styling | Tailwind CSS (standalone CLI) | Utility-first CSS, compiled at build time |
| Interactivity | htmx (served as static asset) | Client-side interactivity, no JS build step |
| ORM | Diesel | Query builder, migrations, schema |
| Database | Postgres (tokio-postgres) | Storage |
| Serialization | Serde | JSON/TOML (de)serialization |
| Async Runtime | Tokio | Runtime, task spawning |
| Observability | tracing + tracing-subscriber | Structured logging |
| Config | toml + figment (or custom) | Layered configuration |
| Connection Pool | deadpool-diesel or bb8 | Async connection management |

### Proc Macro Layer

Autumn's DX lives in its proc macros. These are the minimum set:

**`#[autumn::main]`** — Entry point. Expands into Tokio runtime setup, config loading, connection pool initialization, route discovery, middleware chain, and server startup. This is where "convention" lives — it makes every decision you didn't.

**`#[get("/path")]`, `#[post("/path")]`, etc.** — Route annotations. Expand into Axum handler registrations. The macro handles extractor wiring — if your function takes a `Db`, it gets a connection from the pool. If it takes `Form<T>`, the body is deserialized. The return type determines the response format: `Markup` → HTML, `Json<T>` → JSON. Error types auto-convert to responses (HTML error pages or JSON error bodies, matching the handler's output type).

**`#[derive(Model)]`** — Bridges Diesel schema to API layer. Generates Queryable, Insertable, and optionally Serialize/Deserialize impls. Reduces the "write the same struct three times" problem.

### What Happens at Build Time

1. `build.rs` checks for the Tailwind standalone CLI in `target/` — downloads it if missing
2. Tailwind scans `src/**/*.rs` for class names in Maud templates
3. Optimized, tree-shaken CSS is output to `static/css/autumn.css`

### What Happens at Startup

1. `#[autumn::main]` triggers config loading from `autumn.toml` (with env var overrides)
2. Database connection pool is created from config
3. Proc macros have registered all annotated handlers into an inventory/linkme-based collector
4. Axum router is built from collected routes
5. Static asset serving is mounted (`static/` dir, htmx + Tailwind CSS bundled)
6. Default middleware is applied: request tracing, error handling, CORS (if configured)
7. Health check endpoint is mounted at `/health`
8. Server binds and starts

The developer sees none of this unless they want to.

---

## Convention Over Configuration

### Project Layout

```
my-app/
├── autumn.toml            # Application config
├── Cargo.toml
├── tailwind.config.js     # Optional — Autumn provides a default
├── migrations/            # Diesel migrations
│   └── 2026-03-17-000000_create_users/
│       ├── up.sql
│       └── down.sql
├── static/                # Static assets (htmx + Tailwind auto-managed)
│   ├── css/
│   │   └── autumn.css     # Generated by Tailwind at build time
│   ├── js/
│   └── images/
├── src/
│   ├── main.rs            # Entry point + route handlers
│   ├── models/            # Diesel models (optional organization)
│   └── handlers/          # Route handlers (optional organization)
```

There is no required directory structure beyond `main.rs` and `autumn.toml`. Autumn discovers routes by annotation, not by filesystem convention. You can put everything in `main.rs` or split across 50 files — the framework doesn't care.

### Defaults You Get For Free

- **Connection pooling**: Sized from config, sensible defaults (min 2, max 10)
- **Structured logging**: JSON in production, pretty-print in development
- **Health check**: `GET /health` returns pool status, uptime
- **Error handling**: Diesel errors → appropriate HTTP status codes, Maud-rendered error pages in HTML mode
- **Tailwind CSS**: Auto-downloaded standalone CLI, compiled at build time, tree-shaken
- **htmx**: Bundled and served automatically, no CDN required
- **Static assets**: `static/` directory served at `/static/` with caching headers
- **CORS**: Permissive in dev, locked down in production
- **Graceful shutdown**: SIGTERM handling out of the box
- **Request IDs**: Every request gets a trace ID

### Config Layering

Priority (highest wins):
1. Environment variables (`AUTUMN_SERVER_PORT=8080`)
2. `autumn.toml`
3. Framework defaults

Environment variable naming: `AUTUMN_` prefix, section and key joined by `_`, uppercased. `database.url` → `AUTUMN_DATABASE_URL`.

---

## Escape Hatches

This is the contract: **every default Autumn provides is a trait implementation. Provide your own, and Autumn steps aside.**

### Level 1: Configuration

Override any default via `autumn.toml` or env vars. No code needed.

```toml
[database.pool]
max_connections = 50
min_connections = 5
connection_timeout_seconds = 30
```

### Level 2: Custom Middleware

Add your own Tower middleware to the stack. Autumn's defaults still apply unless you explicitly replace them.

```rust
#[autumn::main]
async fn main() {
    autumn::run()
        .middleware(my_auth_layer())
        .await;
}
```

### Level 3: Raw Axum

Drop down to raw Axum handlers. They mount alongside Autumn-annotated routes with zero friction.

```rust
#[autumn::main]
async fn main() {
    let custom_router = Router::new()
        .route("/legacy", get(my_raw_handler));

    autumn::run()
        .merge(custom_router)
        .await;
}
```

### Level 4: Replace a Subsystem

Implement the trait yourself and Autumn uses yours instead.

```rust
// Autumn provides DatabasePoolProvider with a default impl.
// Implement it yourself to take full control.
impl DatabasePoolProvider for MyCustomPool {
    async fn acquire(&self) -> Result<Connection, AutumnError> {
        // Your custom connection logic
    }
}
```

### Level 5: Don't Use Autumn

Cherry-pick individual crates. Autumn is not a walled garden. Every piece works standalone.

---

## Resolved Design Decisions

### Route Discovery Mechanism — RESOLVED

**`inventory`/`linkme` by default, explicit registration as escape hatch.**

Guiding principle: magic by default, escape hatches when you need them. `inventory`/`linkme` gives developers the Spring Boot experience — annotate a function, it's a route, done. No registration boilerplate.

The escape hatch: if you need fine-grained control over route ordering, conditional registration, or just want to see the wiring explicitly, you can bypass discovery and build the router yourself via `.merge()` (see Escape Hatches, Level 3).

Compile-time and debuggability impact needs evaluation during implementation, but the design commitment is made: discovery is the default path.

### Async Diesel — RESOLVED

**`diesel-async` directly.** No `spawn_blocking`.

The `spawn_blocking` approach creates colored functions by the back door — you're technically async but you're burning a blocking thread per query. That defeats the purpose. `diesel-async` is the honest answer. It's more complex under the hood, but Autumn's proc macros and `Db` extractor hide that complexity from the developer. The user writes `db.await?` and never thinks about it.

### Error Handling Strategy — RESOLVED

**Thiserror for the user. Trait-based mapping for the framework boundary. Autumn's internal error type is opaque. The `?` operator just works — no turbofish, ever.**

The core problems:

1. A framework error *enum* is a closed set. Every new variant is a breaking change that forces updates to every match arm in every test (a real pain point from prior experience).
2. If users have to turbofish every `?` call to tell the compiler which `From<X>` impl to use, the error system is fighting the developer instead of helping them.

The design:

**1. The proc macro rewrites your return type so `?` always works:**

You write:

```rust
#[get("/users")]
async fn list_users(db: Db) -> Markup {
    let users = users::table.load::<User>(&db).await?;
    html! { /* ... */ }
}
```

The macro expands this to:

```rust
async fn list_users(db: Db) -> Result<Markup, AutumnError> {
    let users = users::table.load::<User>(&db).await?;
    Ok(html! { /* ... */ })
}
```

The developer never sees the `Result` wrapper. They return `Markup` or `Json<T>`, use `?` freely, and it compiles.

**2. `AutumnError` accepts anything via a blanket `From` impl:**

```rust
// This is the anyhow trick — one target type, accepts everything.
impl<E: std::error::Error + Send + Sync + 'static> From<E> for AutumnError {
    fn from(err: E) -> Self {
        AutumnError::new(StatusCode::INTERNAL_SERVER_ERROR, err)
    }
}
```

This is why `?` never needs a turbofish. There's exactly one conversion target and it accepts any error. Default status: 500.

**3. `AutumnError` is a struct, not an enum.** It carries a status code, a message, and an optional source — but nobody matches on it. When Autumn adds new internal error kinds, zero downstream code breaks.

**4. Users opt in to status code refinement via `IntoAutumnError`:**

```rust
#[derive(Debug, thiserror::Error)]
enum MyError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error(transparent)]
    Db(#[from] diesel::result::Error),
}

impl IntoAutumnError for MyError {
    fn status(&self) -> StatusCode {
        match self {
            MyError::NotFound => StatusCode::NOT_FOUND,
            MyError::Unauthorized => StatusCode::UNAUTHORIZED,
            MyError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
```

This is entirely optional. If you never implement `IntoAutumnError`, every error is a 500 and `?` still works everywhere. The trait is for refinement, not compilation.

**5. Specialization workaround via proc macro.** The blanket `From` impl and `IntoAutumnError`-aware `From` impl would overlap — Rust doesn't have specialization yet. The solution: the proc macro inspects the error types at compile time. If a type implements `IntoAutumnError`, the macro generates a conversion that captures the status code. If not, it falls through to the blanket `From` (→ 500). No runtime dispatch, no specialization required.

**6. Response format matches the handler's output type.** An error in a `Markup` handler renders a Maud error page. An error in a `Json<T>` handler returns a JSON error body. The user never thinks about this.

### Template Rendering — RESOLVED

Maud (compile-time HTML) + htmx (client-side interactivity) is the default rendering stack. Autumn is application-first: handlers return `Markup` (HTML) by default. JSON is the escape hatch via `Json<T>` return type. The return type is the contract — no extra annotation needed. This means a single Autumn app can serve a complete web UI and a partner/customer API from the same models and database.

### Static Assets, Tailwind & Asset Pipeline

**v1:** Autumn includes Tailwind CSS as a first-class part of the build. No npm. No node_modules. No separate build step.

**How it works:**

1. On first `cargo build`, Autumn's `build.rs` auto-downloads the Tailwind standalone CLI binary (platform-appropriate, cached in `target/`)
2. Tailwind scans `src/**/*.rs` for class names in Maud templates — Tailwind's scanner doesn't care that it's reading Rust files, it just matches string patterns
3. An optimized, tree-shaken CSS file is output to `static/css/autumn.css`
4. The `#[autumn::main]` macro auto-includes it in a base HTML wrapper
5. htmx is also bundled and served automatically

`cargo build` does everything. The developer writes Tailwind classes in Maud and never thinks about CSS tooling.

```rust
html! {
    div class="max-w-4xl mx-auto p-6" {
        h1 class="text-3xl font-bold text-gray-900" { "Users" }
        ul class="mt-4 space-y-2" {
            @for user in &users {
                li class="p-3 bg-white rounded-lg shadow" {
                    a href={"/users/" (user.id)}
                      class="text-blue-600 hover:text-blue-800" {
                        (user.name)
                    }
                }
            }
        }
    }
}
```

Convention project layout:

```
my-app/
├── static/
│   ├── css/
│   │   └── autumn.css   # generated by Tailwind — don't edit
│   ├── js/              # htmx auto-served, user JS if needed
│   └── images/
├── tailwind.config.js   # optional — Autumn provides a default
```

**Escape hatches:**

- Provide your own `tailwind.config.js` to customize the theme, add plugins (DaisyUI, etc.)
- Disable Tailwind entirely via cargo feature: `autumn = { default-features = false, ... }`
- Add your own CSS files in `static/css/` — they're served alongside Tailwind output

**v2+:** Asset pipeline with fingerprinting/hashing for cache busting, optional CDN prefix via config, and image optimization. The config escape hatch:

```toml
[assets]
path = "static"
prefix = "/static"
# cdn = "https://cdn.example.com"   # uncomment for production CDN
# fingerprint = true                 # uncomment for cache-busting hashes
```

### Testing Story — RESOLVED (No Framework Feature Needed)

Rust's built-in testing (`cargo test`, `#[cfg(test)]`, `mod tests`) already solves the problem `@SpringBootTest` exists to solve in Java. No proc macro or framework-level test concept is needed. A convenience `autumn::test::Client` helper that spins up the app against a test database may be worth shipping in v0.2, but it's just a utility, not architecture.

### Migration Management

Diesel CLI handles migrations. Does Autumn wrap it, or just document "use diesel CLI"?

Leaning toward: Autumn runs pending migrations on startup in dev mode (configurable), delegates to Diesel CLI for explicit management.

### Starter / Feature System

Spring Boot's "starters" are dependency bundles with auto-configuration. In Rust, this maps to cargo features that activate configuration modules. For example:

- `autumn = { features = ["redis"] }` → connection pool + config section + extractor
- `autumn = { features = ["jwt"] }` → auth middleware + token extraction

This is a v0.2+ concern but the architecture should support it from day one.

---

## Future Considerations (Don't Build, Don't Block)

These are things Autumn should not implement in v1 but must not design itself into a corner on.

### Content Negotiation

A single handler that returns HTML *or* JSON based on the `Accept` header. The current "return type is the contract" design leaves room for a future `Negotiated<T>` wrapper type:

```rust
#[get("/users/{id}")]
async fn get_user(id: Path<i32>, db: Db) -> Negotiated<User> {
    let user = users::table.find(*id).first::<User>(&db).await?;
    Negotiated(user)  // HTML if Accept: text/html, JSON if Accept: application/json
}
```

This works as long as `Negotiated<T>` can implement `IntoResponse` (it can — Axum's trait is open) and `T` implements both `Serialize` and some `Renderable` trait for Maud output. No v1 design changes needed to keep this door open.

### WebSocket Support

htmx has WebSocket extensions. Axum has native WebSocket support. The intersection is natural but not v1.

### Background Jobs / Scheduled Tasks

Spring Boot's `@Scheduled` is a common pattern. Rust has tokio tasks and cron crates. Worth opinionating on eventually, not yet.