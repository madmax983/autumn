# System Architecture: autumn

**Date:** 2026-03-20
**Architect:** markm
**Version:** 1.0
**Project Type:** library (framework)
**Project Level:** 4
**Status:** Draft

---

## Document Overview

This document defines the internal architecture of the Autumn web framework. Unlike a typical application architecture document, this describes the design of a framework library — crate structure, macro expansion strategy, type hierarchy, build pipeline, and extension points.

**Related Documents:**
- Product Requirements Document: `docs/prd-autumn-2026-03-20.md`
- Product Brief: `docs/product-brief-autumn-2026-03-20.md`
- Technical Brainstorming: `docs/brainstorming-technical-challenges-2026-03-20.md`
- Competitive Research: `docs/research-competitive-technical-2026-03-20.md`

---

## Executive Summary

Autumn is a layered framework built on Axum, organized as a Cargo workspace with three crates: `autumn` (runtime + re-exports), `autumn-macros` (proc macros), and `autumn-cli` (project scaffolding). The architecture follows three principles:

1. **Thin wrappers, not deep rewrites.** Proc macros generate minimal adapter code around user functions. The user's code compiles independently; the macro adds route registration and contextual error handling.

2. **Static dispatch everywhere.** No `Box<dyn>`, no runtime reflection, no dynamic dispatch in the request-handling hot path. All routing and extraction is resolved at compile time.

3. **Escape hatches at every layer.** Every framework opinion is implemented via a trait with a default implementation. Override config → override middleware → mount raw Axum routes → replace subsystems → don't use Autumn.

---

## Architectural Drivers

These NFRs most heavily influence design decisions:

| Driver | NFR | Constraint | Impact |
|--------|-----|------------|--------|
| Error Quality | NFR-006 | All errors must point at user code, not generated code | Drives thin-wrapper macro design, auto `debug_handler` |
| Build Determinism | NFR-005 | `cargo build` never accesses the network | Drives Tailwind CLI management via `autumn-cli` |
| Stable Rust | NFR-003 | No nightly features | Constrains proc macros: no specialization, no `proc_macro_diagnostic` |
| Framework Overhead | NFR-001 | < 1ms overhead per request | Demands static dispatch, zero-cost abstractions |
| Build Time | NFR-002 | Clean < 90s, incremental < 15s | Drives crate separation, minimal proc macro work |
| No Forks | NFR-008 | Must not fork upstream crates | Integration via traits + re-exports, not source modification |

---

## System Overview

### High-Level Architecture

Autumn is a compile-time framework. Most of its value is delivered during compilation (macro expansion, Tailwind CSS generation, type checking). At runtime, it's a thin configuration and wiring layer on top of Axum.

```
┌─────────────────────────────────────────────────────────────────┐
│                        Developer Code                           │
│  #[get("/users")]  async fn list(db: Db) -> AutumnResult<Markup>│
└────────────────────────────┬────────────────────────────────────┘
                             │ proc macro expansion
┌────────────────────────────▼────────────────────────────────────┐
│                      autumn-macros crate                        │
│  • Route annotation macros (#[get], #[post], etc.)              │
│  • #[autumn::main] entry point macro                            │
│  • #[derive(Model)] convenience macro                           │
│  • routes![] collection macro                                   │
└────────────────────────────┬────────────────────────────────────┘
                             │ generates
┌────────────────────────────▼────────────────────────────────────┐
│                       autumn crate (runtime)                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐          │
│  │  Config   │ │   Db     │ │  Error   │ │ Routing  │          │
│  │  System   │ │  Layer   │ │ Handling │ │  Types   │          │
│  └─────┬────┘ └─────┬────┘ └─────┬────┘ └─────┬────┘          │
│        │            │            │             │                │
│  ┌─────▼────────────▼────────────▼─────────────▼──────┐        │
│  │                  App Builder                        │        │
│  │  autumn::app().routes(r).run().await                │        │
│  └─────────────────────┬──────────────────────────────┘        │
│                        │                                        │
│  ┌─────────────────────▼──────────────────────────────┐        │
│  │              Axum + Tower + Tokio                   │        │
│  └────────────────────────────────────────────────────┘        │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                      autumn-cli (binary)                        │
│  • autumn new <name>     (project scaffolding)                  │
│  • autumn setup          (download Tailwind CLI)                │
│  • autumn migrate        (run Diesel migrations)                │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                      build.rs (per-project)                     │
│  • Runs Tailwind CLI (local binary, never downloads)            │
│  • Scans src/**/*.rs for CSS class names                        │
│  • Outputs static/css/autumn.css                                │
└─────────────────────────────────────────────────────────────────┘
```

### Architectural Pattern

**Pattern:** Layered Library with Compile-Time Code Generation

**Rationale:** Autumn is not a microservice or a monolith — it's a library that users add to their Cargo.toml. The "architecture" is the internal structure of the library and the code it generates. The layered approach (macros → types → runtime → Axum) ensures that each layer can be understood and tested independently, and that the escape hatches at each layer work without affecting the layers below.

---

## Crate Structure

### Workspace Layout

```
autumn/
├── Cargo.toml                    # Workspace root
├── autumn/                       # Main crate (runtime + re-exports)
│   ├── Cargo.toml
│   ├── build.rs                  # Tailwind CSS pipeline (template, copied to user projects)
│   └── src/
│       ├── lib.rs                # Re-exports, prelude
│       ├── prelude.rs            # use autumn::prelude::*
│       ├── app.rs                # App builder
│       ├── config.rs             # Configuration loading
│       ├── db.rs                 # Db extractor, pool creation
│       ├── error.rs              # AutumnError, AutumnResult
│       ├── logging.rs            # tracing setup
│       ├── route.rs              # Route type, RouteCollection trait
│       ├── server.rs             # Server startup, graceful shutdown
│       ├── health.rs             # Health check handler
│       └── middleware/
│           ├── mod.rs
│           └── request_id.rs     # Request ID middleware
│
├── autumn-macros/                # Proc macro crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                # Proc macro entry points
│       ├── route.rs              # #[get], #[post], etc.
│       ├── main_macro.rs         # #[autumn::main]
│       ├── model.rs              # #[derive(Model)]
│       └── routes_macro.rs       # routes![]
│
├── autumn-cli/                   # CLI binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── new.rs                # autumn new
│       ├── setup.rs              # autumn setup
│       └── templates/            # Project scaffolding templates
│
└── examples/
    └── todo-app/                 # Non-trivial example
        ├── Cargo.toml
        ├── autumn.toml
        ├── build.rs
        ├── migrations/
        ├── static/
        └── src/
            ├── main.rs
            ├── models.rs
            ├── routes/
            │   ├── mod.rs
            │   ├── todos.rs
            │   └── api.rs
            └── schema.rs
```

### Crate Dependency Graph

```
autumn-cli ─────────► (no runtime dependency on autumn)
                      (uses templates, not code imports)

User's App ─────────► autumn ─────────► autumn-macros
                         │
                         ├──► axum
                         ├──► tokio
                         ├──► diesel + diesel-async
                         ├──► deadpool-diesel
                         ├──► maud
                         ├──► tower-http
                         ├──► tracing + tracing-subscriber
                         ├──► serde + toml
                         └──► thiserror

autumn-macros ──────► syn + quote + proc-macro2
```

**Key decisions:**
- `autumn-cli` does NOT depend on `autumn` at the crate level. It generates project files from templates. This prevents the CLI from pulling in the entire framework dependency tree for a scaffolding operation.
- `autumn` re-exports key types from `axum`, `maud`, `diesel`, etc. so users typically only need `use autumn::prelude::*`.
- `autumn-macros` only depends on proc macro utilities (`syn`, `quote`, `proc-macro2`). It does not depend on `axum` or any runtime crate — it generates code that references `autumn::` paths, not `axum::` paths directly.

---

## Proc Macro Design

This is the highest-risk, most architecturally significant component. Every design decision here is driven by NFR-006 (error quality) and NFR-003 (stable Rust).

### Design Principle: Generate Around, Not Into

The macro never modifies the user's function body. It generates adapter code *around* the function. This ensures:
- Compile errors in user code point at user code
- The user's function is a valid Rust function independent of the macro
- `cargo expand` shows understandable generated code

### Route Annotation Macros (#[get], #[post], etc.)

**Input:**
```rust
#[get("/users/{id}")]
async fn get_user(id: Path<i32>, db: Db) -> AutumnResult<Markup> {
    let user = users::table.find(id.0).first(&mut *db).await?;
    Ok(html! {
        h1 { (user.name) }
    })
}
```

**Expansion (simplified):**
```rust
// 1. User's function is preserved, with debug_handler added in debug mode
#[cfg_attr(debug_assertions, axum::debug_handler(state = autumn::AppState))]
async fn get_user(id: Path<i32>, db: Db) -> AutumnResult<Markup> {
    let user = users::table.find(id.0).first(&mut *db).await?;
    Ok(html! {
        h1 { (user.name) }
    })
}

// 2. Route info function generated for routes![] macro
#[doc(hidden)]
pub fn __autumn_route_info_get_user() -> autumn::route::Route {
    autumn::route::Route {
        method: autumn::reexports::http::Method::GET,
        path: "/users/{id}",
        handler: autumn::reexports::axum::routing::get(get_user),
        name: "get_user",
    }
}
```

**What the macro does (step by step):**

1. **Parse attribute:** Extract HTTP method and path pattern from `#[get("/users/{id}")]`
2. **Validate basics:** Emit `compile_error!` if:
   - Item is not an `async fn`
   - Path is empty or malformed
   - Function has no return type
3. **Add `debug_handler`:** Insert `#[cfg_attr(debug_assertions, axum::debug_handler(state = autumn::AppState))]` on the function
4. **Generate route info:** Create a `__autumn_route_info_{name}()` function that returns a `Route` struct pairing the method + path with an Axum method router pointing to the handler
5. **Emit both:** Output the (annotated) original function and the route info function

**What the macro does NOT do:**
- Does not rewrite the function signature
- Does not wrap the function body in error handling
- Does not generate a separate wrapper function (in v0.1)
- Does not resolve types or check trait implementations

### Error Context via Return Type

The route macro inspects the return type to determine error rendering context:

```rust
// Macro sees: -> AutumnResult<Markup>
// Knows inner type is Markup (HTML) → errors should render as HTML

// Macro sees: -> AutumnResult<Json<Vec<User>>>
// Knows inner type is Json<_> → errors should render as JSON
```

**Implementation:** The `Route` struct includes an `error_context` field set at macro expansion time:

```rust
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    pub handler: MethodRouter<AppState>,
    pub name: &'static str,
}
```

For v0.1, error rendering context is determined by Axum's existing `IntoResponse` implementations. `AutumnError` implements `IntoResponse` and defaults to JSON error bodies. HTML error pages are a v1.0 feature (FR-045).

**v0.1 simplification:** All errors render as JSON responses. This is acceptable because:
- JSON errors are universally parseable (by browsers, API clients, htmx)
- HTML error pages require a design system that doesn't exist yet
- The product brief lists error page overrides as v1.0

### routes![] Macro

**Input:**
```rust
pub fn routes() -> Vec<autumn::route::Route> {
    routes![get_user, list_users, create_user]
}
```

**Expansion:**
```rust
pub fn routes() -> Vec<autumn::route::Route> {
    vec![
        __autumn_route_info_get_user(),
        __autumn_route_info_list_users(),
        __autumn_route_info_create_user(),
    ]
}
```

**What the macro does:**
1. For each identifier in the list, resolve to the `__autumn_route_info_{name}()` function
2. Generate a `vec![]` of calls to those functions
3. If a listed function doesn't have a corresponding route info function, the compiler will error with "function not found" — which is a clear error, not a silent omission

**Module-level pattern:**
```rust
// src/routes/users.rs
mod users {
    use autumn::prelude::*;

    #[get("/users")]
    async fn list(db: Db) -> AutumnResult<Json<Vec<User>>> { ... }

    #[get("/users/{id}")]
    async fn get(id: Path<i32>, db: Db) -> AutumnResult<Json<User>> { ... }

    #[post("/users")]
    async fn create(db: Db, body: Json<NewUser>) -> AutumnResult<Json<User>> { ... }

    pub fn routes() -> Vec<autumn::route::Route> {
        routes![list, get, create]
    }
}

// src/main.rs
#[autumn::main]
async fn main() {
    autumn::app()
        .routes(users::routes())
        .routes(posts::routes())
        .run()
        .await;
}
```

### #[autumn::main] Macro

**Input:**
```rust
#[autumn::main]
async fn main() {
    autumn::app()
        .routes(users::routes())
        .run()
        .await;
}
```

**Expansion:**
```rust
#[tokio::main]
async fn main() {
    autumn::app()
        .routes(users::routes())
        .run()
        .await;
}
```

**v0.1 scope:** `#[autumn::main]` is a thin wrapper around `#[tokio::main]`. Future versions may add:
- Custom panic handler with request context
- Runtime configuration (worker threads, stack size)
- Instrumentation

The real work happens in `autumn::app().run()`, not in the macro. This keeps the macro minimal and the runtime behavior inspectable.

### #[derive(Model)] Macro

**Input:**
```rust
#[derive(Model)]
#[model(table = "users")]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: String,
    pub created_at: NaiveDateTime,
}
```

**Expansion:**
```rust
#[derive(Debug, Clone, Queryable, Selectable, Insertable, Serialize, Deserialize)]
#[diesel(table_name = users)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: String,
    pub created_at: NaiveDateTime,
}
```

**What the macro does:**
1. Parse the struct and `#[model()]` attributes
2. Determine table name: explicit `table = "..."` or infer from struct name (snake_case pluralized)
3. Emit the struct with Diesel + Serde derive macros applied
4. If no `table` attribute, add `#[diesel(table_name = ...)]` with the inferred name

**What it does NOT do:**
- Does not generate Diesel schema (that's `diesel print-schema`)
- Does not handle relations (belongs_to, has_many) — out of scope for v0.1
- Does not generate a "NewUser" insertable variant (consider for v0.2)

---

## Type System Design

### Core Types

```rust
// ── autumn::prelude ──────────────────────────────────────────

// Re-exports for user convenience
pub use crate::app;
pub use crate::db::Db;
pub use crate::error::{AutumnError, AutumnResult};
pub use crate::route::Route;

// Re-exports from upstream crates
pub use axum::extract::{Form, Json, Path, Query, State};
pub use axum::http::StatusCode;
pub use axum::response::IntoResponse;
pub use maud::{html, Markup, PreEscaped};
pub use serde::{Deserialize, Serialize};

// Macro re-exports
pub use autumn_macros::{delete, get, main, post, put, Model};
```

### Error Handling Types

```rust
// ── autumn::error ────────────────────────────────────────────

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Opaque error type for all Autumn handlers.
///
/// Wraps any error implementing `std::error::Error` and converts it
/// to an appropriate HTTP response. Default status is 500.
///
/// # Usage
/// ```
/// async fn handler(db: Db) -> AutumnResult<Json<User>> {
///     let user = users::table.find(1).first(&mut *db).await?; // ? just works
///     Ok(Json(user))
/// }
/// ```
pub struct AutumnError {
    inner: Box<dyn std::error::Error + Send + Sync>,
    status: StatusCode,
}

/// Convenience alias. Every Autumn handler returns this.
pub type AutumnResult<T> = Result<T, AutumnError>;

// ── Blanket conversion: ? works on any Error ─────────────────

impl<E> From<E> for AutumnError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        AutumnError {
            inner: Box::new(err),
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

// ── Status code refinement ───────────────────────────────────

impl AutumnError {
    /// Override the HTTP status code for this error.
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Create a 404 Not Found error.
    pub fn not_found(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        AutumnError {
            inner: Box::new(err),
            status: StatusCode::NOT_FOUND,
        }
    }

    /// Create a 400 Bad Request error.
    pub fn bad_request(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        AutumnError {
            inner: Box::new(err),
            status: StatusCode::BAD_REQUEST,
        }
    }

    /// Create a 422 Unprocessable Entity error.
    pub fn unprocessable(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        AutumnError {
            inner: Box::new(err),
            status: StatusCode::UNPROCESSABLE_ENTITY,
        }
    }
}

// ── HTTP response rendering ──────────────────────────────────

impl IntoResponse for AutumnError {
    fn into_response(self) -> Response {
        // Log the error with full details (not exposed to client)
        tracing::error!(
            status = self.status.as_u16(),
            error = %self.inner,
            "Request failed"
        );

        // v0.1: JSON error response for all handlers
        // v1.0: Context-aware (HTML error pages for HTML handlers)
        let body = serde_json::json!({
            "error": {
                "status": self.status.as_u16(),
                "message": self.status.canonical_reason().unwrap_or("Error"),
            }
        });

        (self.status, axum::Json(body)).into_response()
    }
}
```

**Usage patterns:**

```rust
// Default: ? converts any Error to 500
async fn list(db: Db) -> AutumnResult<Json<Vec<User>>> {
    let users = users::table.load(&mut *db).await?; // 500 on failure
    Ok(Json(users))
}

// Explicit status: .map_err() for non-500 cases
async fn get(id: Path<i32>, db: Db) -> AutumnResult<Json<User>> {
    let user = users::table
        .find(id.0)
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?; // 404 on missing
    Ok(Json(user))
}

// Status override: for custom error types
async fn create(db: Db, body: Json<NewUser>) -> AutumnResult<Json<User>> {
    validate(&body)?; // 500 by default
    // Or: validate(&body).map_err(AutumnError::unprocessable)?; // 422
    let user = diesel::insert_into(users::table)
        .values(&body.0)
        .get_result(&mut *db)
        .await?;
    Ok(Json(user))
}
```

**Why this design:**
- `?` works everywhere with zero ceremony (blanket `From`)
- Status code refinement is opt-in, not required
- The common case (500 for unexpected errors) requires zero boilerplate
- The uncommon case (404, 422) requires one `.map_err()` call
- No custom error enums needed for simple applications
- Advanced users can still define error enums with `thiserror` — they implement `Error`, so the blanket `From` picks them up

### Route Types

```rust
// ── autumn::route ────────────────────────────────────────────

use axum::routing::MethodRouter;
use axum::http::Method;

/// A single route definition: method + path + handler.
///
/// Generated by the `#[get]`/`#[post]`/etc. macros and collected
/// by the `routes![]` macro.
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    pub handler: MethodRouter<AppState>,
    pub name: &'static str,
}

impl Route {
    pub fn new(
        method: Method,
        path: &'static str,
        handler: MethodRouter<AppState>,
        name: &'static str,
    ) -> Self {
        Self { method, path, handler, name }
    }
}
```

### Database Types

```rust
// ── autumn::db ───────────────────────────────────────────────

use axum::extract::FromRequestParts;
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::deadpool::Pool;

/// Shared application state containing the database pool and config.
#[derive(Clone)]
pub struct AppState {
    pub pool: Pool<AsyncPgConnection>,
    pub config: Arc<AutumnConfig>,
}

/// Database connection extractor.
///
/// Declare `db: Db` in your handler signature to get an async
/// database connection from the pool.
///
/// ```
/// #[get("/users")]
/// async fn list(db: Db) -> AutumnResult<Json<Vec<User>>> {
///     let users = users::table.load(&mut *db).await?;
///     Ok(Json(users))
/// }
/// ```
pub struct Db(
    deadpool::managed::Object<diesel_async::pooled_connection::AsyncDieselConnectionManager<AsyncPgConnection>>
);

impl std::ops::Deref for Db {
    type Target = AsyncPgConnection;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl std::ops::DerefMut for Db {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

#[axum::async_trait]
impl FromRequestParts<AppState> for Db {
    type Rejection = AutumnError;

    async fn from_request_parts(
        _parts: &mut http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let conn = state.pool.get().await.map_err(|e| {
            tracing::error!(error = %e, "Failed to acquire database connection");
            AutumnError::from(e).with_status(StatusCode::SERVICE_UNAVAILABLE)
        })?;
        Ok(Db(conn))
    }
}
```

---

## Configuration System

### Three-Layer Config

```
Priority (highest wins):
┌─────────────────────────────┐
│  3. Environment Variables   │  AUTUMN_SERVER__PORT=8080
├─────────────────────────────┤
│  2. autumn.toml             │  [server]
│                             │  port = 3000
├─────────────────────────────┤
│  1. Framework Defaults      │  (compiled into binary)
└─────────────────────────────┘
```

### Config Schema

```rust
// ── autumn::config ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AutumnConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub health: HealthConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,              // default: 3000
    #[serde(default = "default_host")]
    pub host: String,           // default: "127.0.0.1"
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_secs: u64,  // default: 30
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_url")]
    pub url: String,            // default: "postgres://localhost/autumn_dev"
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,       // default: 10
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,  // default: 5
}

#[derive(Debug, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,          // default: "info"
    #[serde(default)]
    pub format: LogFormat,      // default: Auto (pretty in dev, JSON in prod)
}

#[derive(Debug, Deserialize, Default)]
pub enum LogFormat {
    #[default]
    Auto,       // pretty when AUTUMN_ENV != "production", JSON otherwise
    Pretty,
    Json,
}

#[derive(Debug, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_path")]
    pub path: String,           // default: "/health"
}
```

### Loading Algorithm

```rust
pub fn load() -> Result<AutumnConfig, ConfigError> {
    // 1. Start with framework defaults (via serde defaults)
    let mut config_str = String::new();

    // 2. Read autumn.toml if it exists
    if let Ok(contents) = std::fs::read_to_string("autumn.toml") {
        config_str = contents;
    }

    // 3. Parse TOML
    let mut config: AutumnConfig = if config_str.is_empty() {
        AutumnConfig::default()
    } else {
        toml::from_str(&config_str)?
    };

    // 4. Apply AUTUMN_* environment variable overrides
    apply_env_overrides(&mut config);

    // 5. Validate
    config.validate()?;

    Ok(config)
}

fn apply_env_overrides(config: &mut AutumnConfig) {
    // AUTUMN_SERVER__PORT → config.server.port
    if let Ok(val) = std::env::var("AUTUMN_SERVER__PORT") {
        if let Ok(port) = val.parse() {
            config.server.port = port;
        }
    }
    // AUTUMN_DATABASE__URL → config.database.url
    if let Ok(val) = std::env::var("AUTUMN_DATABASE__URL") {
        config.database.url = val;
    }
    // AUTUMN_LOG__LEVEL → config.log.level
    if let Ok(val) = std::env::var("AUTUMN_LOG__LEVEL") {
        config.log.level = val;
    }
    // ... other overrides
}
```

**Why not `figment` or `config` crate?**
- The config surface is small and well-defined
- Custom env var parsing is ~50 lines of code
- Avoids an opaque dependency that could change behavior in minor versions
- Full control over error messages and validation

---

## Application Bootstrap

### App Builder

```rust
// ── autumn::app ──────────────────────────────────────────────

pub fn app() -> AppBuilder {
    AppBuilder {
        routes: Vec::new(),
        layers: Vec::new(),
    }
}

pub struct AppBuilder {
    routes: Vec<Route>,
    layers: Vec<Box<dyn Layer>>, // simplified; actual impl uses Tower layers
}

impl AppBuilder {
    /// Add routes from a module.
    pub fn routes(mut self, routes: Vec<Route>) -> Self {
        self.routes.extend(routes);
        self
    }

    /// Add custom Tower middleware.
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<...> + Clone + Send + Sync + 'static,
    {
        // Store layer for application during run()
        self
    }

    /// Merge a raw Axum router (escape hatch).
    pub fn merge(mut self, router: axum::Router<AppState>) -> Self {
        // Store for merging during run()
        self
    }

    /// Start the server. This is where everything comes together.
    pub async fn run(self) {
        // 1. Load configuration
        let config = config::load().unwrap_or_else(|e| {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        });

        // 2. Initialize logging
        logging::init(&config.log);

        // 3. Log startup banner
        tracing::info!("Autumn v{}", env!("CARGO_PKG_VERSION"));
        tracing::info!("Loading configuration from autumn.toml");

        // 4. Create database pool
        let pool = db::create_pool(&config.database).await.unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to create database pool");
            std::process::exit(1);
        });

        // 5. Build application state
        let state = AppState {
            pool,
            config: Arc::new(config),
        };

        // 6. Build router from collected routes
        let mut router = axum::Router::new();

        if self.routes.is_empty() {
            panic!("No routes registered. Did you forget to call .routes()?");
        }

        for route in &self.routes {
            tracing::info!("{} {}", route.method, route.path);
            router = router.route(route.path, route.handler.clone());
        }

        // 7. Add framework middleware (order matters: outermost first)
        let router = router
            .route(&state.config.health.path, axum::routing::get(health::handler))
            .nest_service("/static", tower_http::services::ServeDir::new("static"))
            .layer(middleware::request_id::RequestIdLayer)
            .layer(middleware::logging::LoggingLayer)
            .with_state(state.clone());

        // 8. Start server with graceful shutdown
        let addr = format!("{}:{}", state.config.server.host, state.config.server.port);
        tracing::info!("Listening on http://{addr}");

        server::run(router, &state.config.server).await;
    }
}
```

### Startup Sequence

```
1. Load config          autumn.toml → env vars → validate
2. Init logging         tracing-subscriber with format from config
3. Log banner           Version, config source
4. Create DB pool       diesel-async + deadpool
5. Build state          Pool + Config wrapped in Arc
6. Build router         Mount routes, log each one
7. Add middleware       Request ID → Logging → Static files → Health check
8. Bind + listen        Axum server with graceful shutdown handler
```

### Graceful Shutdown

```rust
// ── autumn::server ───────────────────────────────────────────

pub async fn run(router: Router, config: &ServerConfig) {
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .expect("Invalid server address");

    let listener = tokio::net::TcpListener::bind(addr).await
        .expect("Failed to bind to address");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(config.shutdown_timeout_secs))
        .await
        .expect("Server error");
}

async fn shutdown_signal(timeout_secs: u64) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, draining connections (timeout: {timeout_secs}s)");
}
```

---

## Build Pipeline

### Tailwind CSS Pipeline (build.rs)

The `build.rs` is a template file generated by `autumn new` into each project. It is NOT part of the `autumn` crate itself — it lives in the user's project.

```rust
// ── build.rs (generated by autumn new) ───────────────────────

fn main() {
    // Only re-run if source files change
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=tailwind.config.js");

    // 1. Find Tailwind CLI
    let tailwind = find_tailwind_cli();

    // 2. Run Tailwind to scan src/**/*.rs and generate CSS
    let status = std::process::Command::new(&tailwind)
        .args([
            "-i", "static/css/input.css",     // Tailwind directives
            "-o", "static/css/autumn.css",     // Output
            "--content", "src/**/*.rs",        // Scan Maud templates
            "--minify",                        // Tree-shake + minify
        ])
        .status()
        .expect("Failed to run Tailwind CLI");

    if !status.success() {
        panic!("Tailwind CSS compilation failed");
    }
}

fn find_tailwind_cli() -> std::path::PathBuf {
    // 1. Check target/autumn/tailwindcss (downloaded by autumn-cli)
    let local = std::path::PathBuf::from("target/autumn/tailwindcss");
    if local.exists() {
        return local;
    }

    // 2. Check PATH
    if let Ok(path) = which::which("tailwindcss") {
        return path;
    }

    // 3. Fail with clear message
    panic!(
        "\n\nTailwind CSS CLI not found!\n\n\
         Install it using one of:\n\
         1. Run `autumn setup` to download it automatically\n\
         2. Install manually: https://tailwindcss.com/blog/standalone-cli\n\
         3. Add `tailwindcss` to your PATH\n\n\
         To disable Tailwind, use `autumn = {{ default-features = false }}`\n"
    );
}
```

### htmx Embedding

htmx is embedded directly in the `autumn` crate as a static byte array:

```rust
// ── autumn (build-time) ──────────────────────────────────────
// htmx.min.js is included in the crate's source tree
// and served via the static asset middleware

pub const HTMX_JS: &[u8] = include_bytes!("../vendor/htmx.min.js");
```

At runtime, the framework serves htmx at `/static/js/htmx.min.js` via a dedicated route (not the static file serving middleware, since it's embedded, not on disk).

---

## Middleware Architecture

### Middleware Stack (outermost → innermost)

```
Request
  │
  ▼
┌─────────────────────────┐
│  Request ID             │  Assigns UUID, adds X-Request-Id header
├─────────────────────────┤
│  Logging                │  Logs method, path, status, duration
├─────────────────────────┤
│  User Middleware        │  Custom Tower layers (.layer())
├─────────────────────────┤
│  Static Files           │  Serves /static/ directory
├─────────────────────────┤
│  Router                 │  Matches routes, extracts params
│  ├─ /health             │  Health check (framework-provided)
│  ├─ /static/js/htmx.js │  Embedded htmx (framework-provided)
│  └─ User routes         │  Handlers registered via routes![]
└─────────────────────────┘
  │
  ▼
Response
```

### Request ID Implementation

```rust
// ── autumn::middleware::request_id ────────────────────────────

use axum::http::{Request, HeaderValue};
use tower::{Layer, Service};
use uuid::Uuid;

#[derive(Clone)]
pub struct RequestIdLayer;

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService { inner }
    }
}

#[derive(Clone)]
pub struct RequestIdService<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for RequestIdService<S>
where
    S: Service<Request<B>, Response = axum::response::Response>,
{
    // ... implementation:
    // 1. Generate UUID
    // 2. Insert into request extensions (for tracing span)
    // 3. Add X-Request-Id header to response
}
```

---

## Health Check

```rust
// ── autumn::health ───────────────────────────────────────────

pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    let pool_status = state.pool.status();
    let healthy = pool_status.available > 0;

    let body = serde_json::json!({
        "status": if healthy { "ok" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "pool": {
            "size": pool_status.size,
            "available": pool_status.available,
            "waiting": pool_status.waiting,
        }
    });

    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status, axum::Json(body))
}
```

---

## Extension Points (Escape Hatches)

### Level 1: Configuration Override

Override any default via `autumn.toml` or `AUTUMN_*` env vars. No code changes needed.

### Level 2: Custom Middleware

```rust
autumn::app()
    .routes(my_routes())
    .layer(tower_http::compression::CompressionLayer::new())
    .layer(tower_http::timeout::TimeoutLayer::new(Duration::from_secs(30)))
    .run()
    .await;
```

### Level 3: Raw Axum Routes

```rust
let custom_router = axum::Router::new()
    .route("/custom", axum::routing::get(custom_handler));

autumn::app()
    .routes(my_routes())
    .merge(custom_router)
    .run()
    .await;
```

### Level 4: Subsystem Replacement (v1.0)

```rust
// Replace the database pool provider
impl autumn::DatabasePoolProvider for MyCustomPool { ... }

// Replace the config loader
impl autumn::ConfigLoader for MyCustomLoader { ... }
```

### Level 5: Don't Use Autumn

Cherry-pick individual crates: Axum, Diesel, Maud, etc. Autumn adds no lock-in because it's a thin layer over published crates.

---

## Non-Functional Requirements Coverage

### NFR-001: Framework Overhead (< 1ms)

**Solution:** All routing is Axum's native router (zero overhead over raw Axum). Extractors are Axum's extractors (re-exported). The only framework overhead is the Request ID middleware (UUID generation: ~100ns) and logging middleware (tracing span: ~200ns). Total: < 500ns.

**Validation:** Benchmark suite comparing Autumn handler vs equivalent raw Axum handler.

### NFR-002: Build Time (clean < 90s, incremental < 15s)

**Solution:** Proc macros generate minimal code. `autumn-macros` crate compiles independently. The Tailwind CLI runs as a subprocess (not compiled into the build). Feature flags allow disabling unused subsystems.

**Validation:** CI tracks build times. Incremental builds only recompile changed files + re-run Tailwind.

### NFR-003: Stable Rust

**Solution:** No nightly features used anywhere. The blanket `From<E: Error>` for `AutumnError` works on stable. Proc macros use only `syn`, `quote`, `proc-macro2` (all stable). No specialization — status code refinement uses explicit methods, not trait dispatch.

**Validation:** CI tests against MSRV and latest stable. `#![deny(unstable_features)]` in all crates.

### NFR-004: Cross-Platform

**Solution:** All code uses platform-agnostic APIs. Tailwind CLI binary selection handles Linux/macOS/Windows. Graceful shutdown uses `tokio::signal` with platform-specific conditionals.

**Validation:** CI tests on ubuntu-latest, macos-latest, windows-latest.

### NFR-005: Build Determinism

**Solution:** `build.rs` never accesses the network. Tailwind CLI is downloaded by `autumn-cli` (explicit user action), not by the build system. htmx is embedded via `include_bytes!`.

**Validation:** Build succeeds in a network-isolated environment (test with `--offline` flag).

### NFR-006: Error Quality

**Solution:** Proc macros generate wrapper code around user functions — user code compiles independently. `#[axum::debug_handler]` auto-applied in debug builds. Macros emit `compile_error!` for detectable mistakes. Runtime errors logged with request ID and context.

**Validation:** Test suite includes "bad handler" examples that verify error messages point at user code.

### NFR-007: Code Quality

**Solution:** CI enforces `cargo fmt`, `cargo clippy` with pedantic lints, and > 85% test coverage. No `unwrap()` in library code. `thiserror` for all error types. Doc comments on all public items.

**Validation:** CI gates on all quality checks.

### NFR-008: No Upstream Forks

**Solution:** All upstream crates used via crates.io published versions. Autumn's types wrap upstream types (e.g., `Db` wraps `deadpool::Object<AsyncPgConnection>`). Integration is via traits and re-exports, never source modification.

**Validation:** `Cargo.toml` audit — no `git = "..."` dependencies.

### NFR-009: Dependency Auditing

**Solution:** `cargo audit` in CI. Dependabot or Renovate for dependency updates. Crate selection favors well-maintained crates with active security response.

**Validation:** CI fails on known vulnerabilities.

### NFR-010: Time To First Endpoint (< 5 min)

**Solution:** `autumn new` generates a complete, running project. First compile includes Tailwind CSS generation. Sample routes demonstrate all core features. No configuration needed for the happy path.

**Validation:** Timed test from clean install to running server.

### NFR-011: Documentation Completeness

**Solution:** Every public type, trait, and macro has doc comments with examples. Getting started guide covers full workflow. Tutorial builds a realistic app. Examples are compiled as part of CI.

**Validation:** `#![deny(missing_docs)]` on public items. Doc examples tested by `cargo test`.

### NFR-012: Licensing

**Solution:** MIT OR Apache-2.0 dual license. All dependencies checked for license compatibility. No GPL/AGPL/SSPL dependencies.

**Validation:** `cargo deny check licenses` in CI.

---

## Testing Strategy

### Test Pyramid

```
           ╱╲
          ╱  ╲
         ╱ E2E╲         autumn-cli: autumn new → cargo build → cargo run → curl
        ╱──────╲
       ╱ Integ. ╲       Full request cycle: HTTP request → handler → DB → response
      ╱──────────╲
     ╱   Unit     ╲     Config parsing, error conversion, route registration
    ╱──────────────╲
   ╱  Proc Macro    ╲   Macro expansion tests (compile-pass, compile-fail)
  ╱──────────────────╲
```

### Test Categories

**1. Proc Macro Tests (autumn-macros)**
- **Expansion tests:** Verify macro output matches expected code (using `macrotest` or `trybuild`)
- **Compile-fail tests:** Verify that bad input produces clear `compile_error!` messages
- **Examples:** missing async, non-function item, empty path, invalid method

```rust
// tests/compile_fail/missing_async.rs
#[autumn::get("/test")]
fn not_async() -> String { // Should fail: not async
    "hello".to_string()
}

// tests/compile_fail/missing_async.stderr
// error: #[get] handlers must be async functions
```

**2. Unit Tests (autumn)**
- Config loading (defaults, TOML parsing, env var override)
- Error type conversion (blanket From, status code methods)
- Route type construction
- Health check response format

**3. Integration Tests (autumn)**
- Full HTTP request cycle using `axum::test::TestClient`
- Route registration via `routes![]` → router → request → response
- Db extractor with test database
- Error handling: correct status codes, correct response format
- Static file serving

```rust
#[tokio::test]
async fn test_json_handler_returns_json() {
    let app = test_app()
        .routes(routes![test_json_handler])
        .build();

    let response = app.get("/test").send().await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
}
```

**4. E2E Tests (autumn-cli)**
- `autumn new test-app` → verify project structure
- `cargo build` in generated project → verify compilation
- `cargo run` + HTTP request → verify running server

### Test Database Strategy

Integration tests use a real Postgres database (not mocks):
- CI runs Postgres via service container
- Each test creates a unique database or uses transactions that roll back
- Diesel migrations run before tests

---

## Development Architecture

### Code Organization Conventions

- One module per file (no inline submodules for public API)
- `mod.rs` only for re-exports, not logic
- Tests in `tests/` directory (integration) or inline `#[cfg(test)]` (unit)
- Examples are complete, runnable applications

### CI/CD Pipeline

```
┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐
│ cargo fmt│───►│  clippy  │───►│  test    │───►│ publish  │
│  --check │    │ pedantic │    │ (3 OS)   │    │ crates.io│
└──────────┘    └──────────┘    └──────────┘    └──────────┘
                                     │
                              ┌──────┴──────┐
                              │  coverage   │
                              │  (tarpaulin)│
                              └─────────────┘
```

**CI matrix:**
- OS: ubuntu-latest, macos-latest, windows-latest
- Rust: MSRV, stable, beta
- Postgres: via service container (Linux) or pre-installed (macOS/Windows)

---

## Requirements Traceability

### Functional Requirements → Components

| FR | Name | Component(s) |
|----|------|-------------|
| FR-001 | CLI Installation | autumn-cli |
| FR-002 | Project Scaffolding | autumn-cli templates |
| FR-003 | Tool Management | autumn-cli setup |
| FR-004 | Crate Structure | Workspace layout |
| FR-005 | Route Macros | autumn-macros::route |
| FR-006 | Debug Handler | autumn-macros::route |
| FR-007 | routes![] Macro | autumn-macros::routes_macro |
| FR-008 | Entry Point | autumn-macros::main_macro + autumn::app |
| FR-009 | DB Pool | autumn::db |
| FR-010 | Db Extractor | autumn::db::Db |
| FR-011 | #[derive(Model)] | autumn-macros::model |
| FR-012 | Path Extractor | Re-export from axum |
| FR-013 | Form Extractor | Re-export from axum |
| FR-014 | JSON Extractor | Re-export from axum |
| FR-015 | JSON Response | Re-export from axum |
| FR-016 | AutumnError | autumn::error |
| FR-017 | Blanket From | autumn::error |
| FR-018 | Custom Status | autumn::error |
| FR-019 | Return Type Contract | autumn::error (AutumnResult) |
| FR-020 | Maud Integration | Re-export from maud |
| FR-021 | Tailwind Pipeline | build.rs template |
| FR-022 | htmx Integration | autumn::vendor (embedded) |
| FR-023 | Static Assets | autumn::app (tower-http) |
| FR-024 | Config File | autumn::config |
| FR-025 | Env Overrides | autumn::config |
| FR-026 | Defaults | autumn::config |
| FR-027 | Logging | autumn::logging |
| FR-028 | Health Check | autumn::health |
| FR-029 | Graceful Shutdown | autumn::server |
| FR-030 | Request ID | autumn::middleware::request_id |
| FR-031–035 | Documentation | docs/, README, cargo doc |
| FR-036 | CI | .github/workflows/ |
| FR-037 | crates.io | Cargo.toml metadata |
| FR-038 | Feature Flags | Cargo.toml features |
| FR-039–048 | v1.0 Features | Various (Should Have) |

### NFR → Architecture Solution

| NFR | Name | Solution |
|-----|------|----------|
| NFR-001 | Framework Overhead | Static dispatch, Axum-native routing |
| NFR-002 | Build Time | Crate separation, minimal macro work |
| NFR-003 | Stable Rust | No nightly features, no specialization |
| NFR-004 | Cross-Platform | CI matrix, platform conditionals |
| NFR-005 | Build Determinism | No network in build.rs |
| NFR-006 | Error Quality | Thin wrappers, debug_handler, compile_error! |
| NFR-007 | Code Quality | CI gates, pedantic lints |
| NFR-008 | No Forks | Re-exports + trait wrappers |
| NFR-009 | Dependency Audit | cargo audit + deny |
| NFR-010 | Time To First Endpoint | autumn new generates running app |
| NFR-011 | Doc Completeness | deny(missing_docs), doc tests |
| NFR-012 | Licensing | MIT/Apache-2.0, cargo deny |

---

## Trade-offs & Decision Log

### TD-001: routes![] vs Auto-Discovery

**Decision:** Use explicit `routes![]` macro (Rocket pattern) instead of linker-based auto-discovery (inventory/linkme).

**Trade-off:**
- ✓ Gain: Zero silent failure risk, cross-platform reliable, compile-time validation
- ✗ Lose: One line of registration per module (instead of zero)

**Rationale:** linkme has a confirmed cross-crate bug (rust-lang/rust#67209) that silently drops routes. For a production framework, reliability trumps convenience. Rocket has proven this pattern works at scale.

### TD-002: Thin Wrapper vs Deep Rewrite (Proc Macros)

**Decision:** Generate wrapper code around user functions, don't modify function body or signature.

**Trade-off:**
- ✓ Gain: Errors point at user code, functions compile independently, `cargo expand` is readable
- ✗ Lose: User must write `AutumnResult<T>` explicitly (no silent return type rewriting)

**Rationale:** NFR-006 (error quality) is the top architectural driver. Inscrutable proc macro errors are the #1 DX killer in Rust frameworks. The explicit return type is a small price for reliable error messages.

### TD-003: Blanket From + Explicit Status vs Specialization

**Decision:** Use blanket `From<E: Error>` for `?` operator (always 500), with explicit `.map_err()` for other status codes.

**Trade-off:**
- ✓ Gain: Works on stable Rust, `?` works everywhere with zero ceremony
- ✗ Lose: Non-500 errors require `.map_err(AutumnError::not_found)` (one method call)

**Rationale:** Specialization is not available on stable Rust. The blanket `From` covers 90% of error handling (unexpected errors → 500). The 10% case (expected errors → 404, 422) is handled by explicit methods, which is actually better for readability — it makes the intent visible.

### TD-004: JSON-Only Errors in v0.1

**Decision:** `AutumnError` renders JSON error responses for all handlers in v0.1. HTML error pages are a v1.0 feature.

**Trade-off:**
- ✓ Gain: Simpler error type, no context-tracking needed, JSON is universally parseable
- ✗ Lose: HTML handlers get JSON errors (not ideal UX)

**Rationale:** Contextual error rendering requires either: (a) the macro tracking return types and generating different error wrappers, or (b) request-time content negotiation. Both add complexity. JSON errors are acceptable for v0.1 — htmx can parse JSON responses, and browsers display JSON readably. HTML error pages are a polish feature.

### TD-005: toml + Custom Env Parsing vs figment/config

**Decision:** Use `toml` crate with hand-written env var override logic instead of `figment` or `config`.

**Trade-off:**
- ✓ Gain: No opaque dependency, full control over error messages, minimal dependency tree
- ✗ Lose: ~50 lines of env var parsing code to maintain

**Rationale:** The config surface is small (4 sections, ~15 fields). `figment` and `config` are powerful but add dependencies and abstraction that isn't needed. Custom code is easier to debug and produces better error messages.

### TD-006: autumn-cli Has No Runtime Dependency on autumn

**Decision:** `autumn-cli` generates projects from templates (string substitution), not from the `autumn` crate's types.

**Trade-off:**
- ✓ Gain: CLI installs fast (small binary), doesn't pull in Axum/Diesel/Maud dependencies
- ✗ Lose: Template and crate can drift if not tested together

**Rationale:** `cargo install autumn-cli` should take seconds, not minutes. The CLI's job is to generate files, not to run Autumn code. Template drift is mitigated by E2E tests that scaffold a project and build it.

### TD-007: Db Extractor as Newtype Wrapper

**Decision:** `Db` is a newtype around the deadpool connection object, implementing `Deref`/`DerefMut` to `AsyncPgConnection`.

**Trade-off:**
- ✓ Gain: Clean API (`db: Db` in handler), hides pool implementation details
- ✗ Lose: Users can't directly access pool features without going through `Db`

**Rationale:** The extractor pattern (declare what you need in the function signature) is Autumn's core DX innovation. `Db` should be the simplest possible way to get a database connection. Advanced pool access can go through `State<AppState>`.

---

## Open Issues & Risks

1. **diesel-async pool crate selection:** deadpool-diesel vs bb8 vs custom. Needs a spike to determine which has the best async story with diesel-async. Deadpool is the working assumption.

2. **Tailwind v4 standalone CLI compatibility:** Must verify that Tailwind v4's standalone CLI works with `--content "src/**/*.rs"` for scanning Maud templates. The class detection patterns may need customization.

3. **`#[derive(Model)]` table name inference:** Pluralization in English is irregular ("person" → "people", "mouse" → "mice"). May need to use the `heck` crate or require explicit table names.

4. **Feature flag granularity:** What's the right level? `tailwind` + `htmx` + `maud` as separate features, or a single `frontend` feature? Too granular increases test matrix.

5. **Error message quality in practice:** The thin-wrapper approach is sound in theory, but real-world error messages need to be tested with actual developer scenarios. Budget time for error message iteration.

---

## Assumptions & Constraints

**Assumptions** (from PRD, carried forward):
- Axum remains the Rust HTTP framework winner (Confidence: High)
- diesel-async is production-ready (Confidence: Medium-High)
- `routes![]` explicit registration is acceptable DX (Confidence: High — Rocket precedent)
- Tailwind standalone CLI remains available (Confidence: High)

**Constraints** (from product brief, carried forward):
- Single developer, 10-20 hours/week
- Stable Rust only
- Postgres only
- No upstream forks
- Zero budget

---

## Future Considerations

These architectural decisions deliberately leave room for future expansion:

1. **Contextual error rendering (v1.0):** The `Route` struct can be extended with an `error_context` field. The macro can set it based on return type. A middleware can then intercept errors and render HTML vs JSON based on context.

2. **Content negotiation (post-v1.0):** A `Negotiated<T>` return type could inspect the Accept header. The type system supports this — `T: Serialize + Into<Markup>` would enable dual rendering.

3. **WebSocket support:** `#[ws("/path")]` macro could generate a WebSocket handler. Axum has native WebSocket support. The Route type would need a variant for upgrade handlers.

4. **Background jobs:** A `#[schedule("0 * * * *")]` macro could register functions with a job scheduler. The AppBuilder would need a `.jobs()` method. Consider tokio-cron-scheduler.

5. **Starter features:** `autumn = { features = ["redis"] }` could activate a Redis connection pool + `Redis` extractor. The trait-based subsystem design (Level 4 escape hatch) supports this naturally.

6. **Linker-based auto-discovery:** If rust-lang/rust#67209 is fixed, auto-discovery could be offered as an opt-in feature flag alongside `routes![]`. The architecture supports both — `routes![]` generates `Vec<Route>`, and auto-discovery would do the same via a different mechanism.

---

## Approval & Sign-off

**Review Status:**
- [ ] Product Owner (Mark)
- [ ] Architecture Review (self-review after implementation spike)

---

## Revision History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-03-20 | markm | Initial architecture informed by PRD, brainstorming, and research |

---

## Next Steps

### Phase 4: Sprint Planning & Implementation

Run `/bmad:sprint-planning` to:
- Break epics into detailed user stories
- Estimate story complexity
- Plan sprint iterations
- Begin implementation following this architectural blueprint

**Key Implementation Principles:**
1. Start with EPIC-002 (Route System) — it's the highest-risk, load-bearing foundation
2. Build the example app alongside the framework (visible reward, integration test)
3. Test proc macros with compile-fail tests from day one
4. Use the 3-month gut check: Do macros work? Does Db work? Would you use this?

---

**This document was created using BMAD Method v6 - Phase 3 (Solutioning)**

*To continue: Run `/bmad:workflow-status` to see your progress and next recommended workflow.*
