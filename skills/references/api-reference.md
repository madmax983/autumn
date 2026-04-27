# autumn-web API Reference

Full type exports, prelude contents, and dependency details for autumn-web 0.2.

## lib.rs Re-Exports

These are the top-level items exported from `autumn_web`:

### Functions
- `autumn_web::app()` → `AppBuilder` — primary entry point

### Types
- `Db` — Async Postgres connection extractor (requires `db` feature)
- `AutumnError` — Framework error type with HTTP status codes
- `AutumnResult<T>` — `Result<T, AutumnError>`
- `Page<T>` — Paginated list response wrapper
- `PageRequest` — Pagination query params extractor (`{ page, size }`)
- `Valid<T>` — Auto-validating extractor wrapper
- `Validated<T>` — Proof that T passed validation

### Proc Macros
| Macro | Purpose |
|-------|---------|
| `#[get("/path")]` | GET route handler |
| `#[post("/path")]` | POST route handler |
| `#[put("/path")]` | PUT route handler |
| `#[delete("/path")]` | DELETE route handler |
| `routes![...]` | Collect route handlers into `Vec<Route>` |
| `#[autumn_web::main]` | Tokio runtime setup |
| `#[model(table = "...")]` | Diesel model derives |
| `#[repository]` | Generated CRUD repository |
| `#[service]` | Service definition macro |
| `#[static_get("/path")]` | Pre-rendered static route |
| `static_routes![...]` | Collect static routes |
| `tasks![...]` | Collect scheduled tasks |
| `#[scheduled(every = "...")]` | Background task schedule |
| `#[secured]` / `#[secured("role")]` | Auth + role guard |
| `#[cached]` | Memoize function by arguments |
| `#[ws("/path")]` | WebSocket route (requires `ws` feature) |
| `#[api_doc]` | OpenAPI annotation |
| `#[oauth2_callback]` | OAuth2 callback handler |

### Maud Re-Exports
- `Markup` — Rendered HTML fragment
- `PreEscaped` — Render string verbatim
- `html!` — HTML templating macro

### Extractors (re-exported)
- `Json`, `Path`, `Form`, `Query` — Axum extractors
- `State` — from `axum::extract::State`

### Constants
- `HTMX_JS_PATH` — Path to bundled htmx.min.js
- `HTMX_CSRF_JS_PATH` — Path to CSRF-aware htmx helper
- `HTMX_VERSION` — Embedded htmx version string

## prelude.rs Contents

`use autumn_web::prelude::*;` brings into scope:

**Route macros**: `get`, `post`, `put`, `delete`, `routes`, `main`, `scheduled`,
`secured`, `service`, `static_get`, `static_routes`, `tasks`, `cached`, `api_doc`,
`oauth2_callback`, `ws` (with `ws` feature)

**Rendering**: `Markup`, `PreEscaped`, `html!`

**Extractors**: `Db`, `Form`, `Json`, `Path`, `Query`, `State`, `Session`, `Auth`,
`CsrfToken`, `PageRequest`, `Page`, `Valid`, `ValidateExt`, `Validated`,
`Flash`/`FlashLevel`/`FlashMessage` (with `flash`), `Multipart` (with `multipart`),
`HxRequest`/`HxResponseExt`/`HTMX_JS_PATH`/`HTMX_CSRF_JS_PATH` (with `htmx`),
`Sse`/`Event` (SSE support)

**Error handling**: `AutumnError`, `AutumnResult`, `AuditEvent`, `AuditStatus`

**Hooks** (with `db`): `DraftField`, `FieldDiff`, `MutationContext`, `MutationHooks`,
`MutationOp`, `Patch`, `UpdateDraft`

**State**: `AppState`

## Cargo.toml Features

```toml
[features]
default = ["maud", "htmx", "tailwind", "db", "cache-moka"]
ws = ["dep:tokio-stream"]
flash = []
cache-moka = ["dep:moka"]
maud = ["dep:maud"]
htmx = []
multipart = ["axum/multipart"]
tailwind = []
oauth2 = ["dep:reqwest", "dep:jsonwebtoken"]
openapi = []
db = ["dep:deadpool", "dep:diesel", "dep:diesel-async", "dep:diesel_migrations",
      "dep:libsqlite3-sys", "dep:pq-sys", "diesel/postgres"]
test-support = ["dep:testcontainers", "dep:testcontainers-modules"]
telemetry-otlp = ["dep:opentelemetry", "dep:opentelemetry_sdk",
                   "dep:opentelemetry-otlp", "dep:tracing-opentelemetry"]
redis = ["dep:redis"]
```

## Workspace Dependencies (pinned versions)

These are the versions used in the autumn workspace root Cargo.toml:

```toml
axum = { version = "0.8", features = ["macros", "ws"] }
diesel = { version = "2", features = ["sqlite"] }
pq-sys = { version = "0.7", features = ["bundled_without_openssl"] }
diesel-async = { version = "0.8", features = ["deadpool", "postgres"] }
diesel_migrations = "2"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
maud = { version = "0.27", features = ["axum"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "fs", "trace"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
validator = { version = "0.20", features = ["derive"] }
bcrypt = "0.19"
chrono = { version = "0.4", features = ["serde"] }
redis = { version = "1.2.0", features = ["aio", "tokio-comp", "connection-manager"] }
tokio-cron-scheduler = { version = "0.15", features = ["signal"] }
uuid = { version = "1", features = ["v4"] }
testcontainers = "0.27"
testcontainers-modules = { version = "0.15", features = ["postgres", "redis"] }
```

## Error Constructors

`AutumnError` provides these convenience constructors:
- `AutumnError::not_found_msg(msg)` — 404
- `AutumnError::bad_request_msg(msg)` — 400
- `AutumnError::unauthorized()` — 401
- `AutumnError::internal_server_error()` — 500

## reexports Module

`autumn_web::reexports` provides direct access to upstream crates used in macro-generated
code: `axum`, `chrono`, `diesel` (db), `diesel_async` (db), `http`, `tokio`,
`tokio_util`, `tracing`, `validator`.
