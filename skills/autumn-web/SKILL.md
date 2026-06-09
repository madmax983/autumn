---
name: autumn-web
description: >
  Use when building, debugging, documenting, or upgrading Rust web applications
  with autumn-web, autumn-cli, or first-party Autumn crates; also use for
  Autumn route/model/repository/job/webhook/admin macros, AppBuilder setup,
  Maud + htmx server-rendered UI, Diesel async Postgres, and Autumn 0.5.x
  migration or release work.
---

# autumn-web - Rust Web Framework

**Repository**: https://github.com/madmax983/autumn
**Branch**: `trunk-dev`
**Current release**: 0.5.0 (2026-06-04) | **Edition**: 2024 | **MSRV**: 1.88.0
**Author**: madmax983

autumn-web is a Spring Boot-style web framework for Rust, built on Axum. It
assembles Axum, Diesel, Maud, htmx, Tailwind, Tokio, tracing, and production
defaults into a convention-over-configuration stack with proc-macro ergonomics.

## Read these references

This file is the quick operating guide. Load the adjacent reference files only
when their details matter:

- `references/api-reference.md` - release-line API map, proc macros,
  feature flags, AppBuilder methods, config env names, and dependency versions.
- `references/examples.md` - official 0.5.0 example patterns for minimal apps,
  CRUD, production-ish jobs, Redis channels, S3 storage plugins, and signed
  webhooks. Use this before generating full app code.

## Crate naming trip wires

| Concept | Name |
|---|---|
| Main library crate on crates.io | `autumn-web` |
| Rust import path | `autumn_web::` |
| Workspace member directory | `autumn/` |
| CLI crate | `autumn-cli` |
| CLI binary | `autumn` |
| Proc macro crate | `autumn-macros` |
| Admin plugin crate | `autumn-admin-plugin` |
| S3 storage plugin crate | `autumn-storage-s3` |
| Redis cache plugin crate | `autumn-cache-redis` |
| Main entry macro | `#[autumn_web::main]`, not `#[autumn::main]` |

The name `autumn` is the CLI binary, not the framework crate. In code, import
from `autumn_web::prelude::*`.

## Project shape

```text
my-app/
├── src/
│   ├── main.rs        # AppBuilder, migrations, routes, jobs, tasks, plugins
│   ├── models.rs      # Diesel models or #[model]
│   ├── schema.rs      # Diesel table! definitions
│   ├── routes/        # #[get], #[post], #[ws], #[static_get] handlers
│   ├── jobs.rs        # #[job] request-triggered background work
│   └── tasks.rs       # #[scheduled] and #[task] operational work
├── migrations/
├── static/
├── Cargo.toml
├── autumn.toml
└── autumn-dev.toml    # legacy profile file; [profile.dev] also works
```

## Cargo.toml

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2024"

[dependencies]
autumn-web = { version = "0.5", features = ["db", "htmx", "maud"] }
chrono = { version = "0.4", features = ["serde"] }
diesel = { version = "2", features = ["postgres", "chrono"] }
diesel-async = { version = "0.8", features = ["postgres"] }
diesel_migrations = "2"
maud = { version = "0.27", features = ["axum"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
validator = { version = "0.20", features = ["derive"] }
```

Use `pq-sys = { version = "0.7", features = ["bundled_without_openssl"] }`
when avoiding a system libpq install.

For a reddit-clone-style app with live feeds, file uploads, and blob variants:
```toml
autumn-web = { version = "0.5", features = [
    "mail",       # transactional email + mailer previews
    "ws",         # WebSocket routes, SSE, broadcast channels
    "presence",   # Presence extractor for online-user tracking
    "storage",    # BlobStore + Blob columns + signed URLs
    "multipart",  # multipart/form-data file uploads
    "redis",      # Redis sessions, channels, and job backend
    "variants",   # blob.variant(...) image transformation
] }
```

## Feature flags

Defaults: `maud`, `htmx`, `tailwind`, `db`, `cache-moka`.

| Feature | Purpose |
|---|---|
| `ws` | WebSocket routes, SSE helpers, local/Redis broadcast channels |
| `flash` | Flash messages |
| `multipart` | Multipart uploads |
| `redis` | Redis sessions, channels, jobs, webhook replay, and integration points |
| `oauth2` | OAuth2/OIDC helpers and `autumn generate auth --oauth` scaffolding |
| `openapi` | OpenAPI route metadata and spec generation |
| `mcp` | Project typed JSON endpoints as MCP tools; implies `openapi` |
| `markdown` | Markdown rendering with frontmatter and static-site support |
| `telemetry-otlp` | OpenTelemetry OTLP export |
| `test-support` | Testcontainers-backed `TestApp`, `TestClient`, and `TestDb` |
| `i18n` | Locale extractor and compile-time checked translations |
| `storage` | `BlobStore`, local storage, `Blob` columns, signed URLs |
| `mail` | Transactional email, mailer macros, previews, deferred delivery |
| `seed` | `SeedContext` for seed binaries |
| `system-info` | Optional system information in actuator surfaces |

For S3 storage add `autumn-storage-s3 = "0.5"`; `storage-s3` is no longer an
`autumn-web` feature. For a shared Redis cache add `autumn-cache-redis = "0.5"`.

## main.rs pattern

```rust
mod jobs;
mod routes;
mod schema;
mod tasks;

use autumn_web::migrate::{embed_migrations, EmbeddedMigrations};
use autumn_web::prelude::*;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![routes::index, routes::create_post])
        .tasks(tasks![tasks::refresh_rankings])
        .jobs(jobs![jobs::send_welcome_email])
        .run()
        .await;
}
```

Use `.one_off_tasks(one_off_tasks![...])` for `#[task]` handlers invoked by
`autumn task <name>`.

## AppBuilder API

| Method | Purpose |
|---|---|
| `.routes(routes![...])` | Register route handlers |
| `.static_routes(static_routes![...])` | Register `#[static_get]` routes for `autumn build` |
| `.tasks(tasks![...])` | Register scheduled `#[scheduled]` work |
| `.jobs(jobs![...])` | Register request-triggered `#[job]` work |
| `.one_off_tasks(one_off_tasks![...])` | Register operational `#[task]` commands |
| `.migrations(MIGRATIONS)` | Register embedded Diesel migrations |
| `.plugin(plugin)` / `.plugins((...))` | Install first- or third-party plugins |
| `.openapi(config)` | Configure OpenAPI generation |
| `.policy::<R, _>(policy)` / `.scope::<R, _>(scope)` | Register repository API authorization |
| `.scoped(prefix, layer, routes)` | Mount a scoped route group |
| `.merge(router)` / `.nest(path, router)` | Attach raw Axum routers |
| `.layer(layer)` | Add Tower middleware |
| `.error_pages(renderer)` / `.exception_filter(filter)` | Customize error rendering |
| `.with_config_loader(loader)` | Replace TOML + env config loading |
| `.with_pool_provider(provider)` | Replace database pool creation |
| `.with_session_store(store)` | Replace sessions |
| `.with_channels_backend(backend)` | Replace broadcast channels |
| `.with_blob_store(store)` | Install a file storage backend |
| `.with_cache_backend(cache)` | Install a cache backend |
| `.with_mail_delivery_queue(queue)` | Install durable deferred mail |
| `.with_audit_sink(sink)` | Install structured audit sink |
| `.run()` | Launch the server |

## Route macros

```rust
#[get("/posts")]
async fn list(db: Db) -> AutumnResult<Markup> { /* ... */ }

#[get("/posts/{id}")]
async fn show(Path(id): Path<i64>, db: Db) -> AutumnResult<Markup> { /* ... */ }

#[post("/posts")]
#[secured]
async fn create(db: Db, Valid(Form(input)): Valid<Form<CreatePost>>) -> AutumnResult<Markup> {
    /* ... */
}

#[patch("/posts/{id}")]
async fn patch(Path(id): Path<i64>, db: Db) -> AutumnResult<Markup> { /* ... */ }

#[delete("/posts/{id}")]
async fn delete_post(Path(id): Path<i64>, db: Db) -> AutumnResult<Markup> { /* ... */ }

#[static_get("/about")]
async fn about() -> Markup { html! { h1 { "About" } } }

#[ws("/socket")]
async fn ws() -> impl autumn_web::ws::WsHandler {
    |mut socket: autumn_web::ws::WebSocket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            if let autumn_web::ws::Message::Text(text) = msg {
                socket.send(autumn_web::ws::Message::Text(text)).await.ok();
            }
        }
    }
}
```

Route functions are collected with `routes![...]`. Static routes also need
`static_routes![...]` so `autumn build` can pre-render them.

## Models and repositories

Autumn uses Diesel + diesel-async for Postgres. Primary keys are `i64` /
`BIGSERIAL`; do not use UUIDs as primary keys. Add UUIDs as separate columns
when external correlation needs them.

```rust
#[model(table = "posts")]
#[derive(Validate)]
pub struct Post {
    pub id: i64,
    #[validate(length(min = 1, max = 500))]
    pub title: String,
    pub body: String,
}
```

Repository-generated APIs in production must either declare a policy or be
explicitly acknowledged in config:

```rust
#[repository(Post, api = "/api/posts", policy = PostPolicy, scope = PostScope)]
pub trait PostRepository {}
```

```toml
[security]
allow_unauthorized_repository_api = true # only when intentional
```

## Security and auth

```rust
#[get("/dashboard")]
#[secured]
async fn dashboard(session: Session) -> AutumnResult<Markup> { /* ... */ }

#[get("/admin")]
#[secured("admin")]
async fn admin_panel() -> AutumnResult<Markup> { /* ... */ }

// Record-level auth on repository-generated REST endpoints:
#[repository(Post, api = "/api/posts", policy = PostPolicy, scope = PostScope)]
pub trait PostRepository {}

// Manual handler: load the record first, then check inline.
// #[authorize] is used by the repository macro; for manual handlers
// the pattern is explicit ownership checks in the body:
#[post("/posts/{id}")]
#[secured]
async fn update_post(Path(id): Path<i64>, mut db: Db, session: Session) -> AutumnResult<Markup> {
    let user_id: i64 = session.get("user_id").await
        .ok_or_else(|| AutumnError::unauthorized())?;
    let post = find_post(&mut *db, id).await?;
    if post.user_id != user_id {
        return Err(AutumnError::forbidden_msg("not your post"));
    }
    /* ... */
    Ok(html! { "updated" })
}
```

In `prod` / `production`, configure a stable signing secret or startup fails:

```bash
export AUTUMN_SECURITY__SIGNING_SECRET="$(openssl rand -hex 32)"
```

For rotation, set `[security.signing_secret].previous_secrets` until old
cookies, CSRF tokens, flash state, and signed storage URLs expire.

## OAuth2/OIDC scaffolding

OAuth2/OIDC social login is in the 0.5.0 line. Do not repeat the stale
changelog claim that it was reverted; the revert was followed by a reapply and
review fixes. Prefer the current tree and `docs/guide/oauth.md` over that old
summary line.

```bash
autumn generate auth User --oauth github,google
```

The generator creates `src/routes/oauth.rs`, an `oauth_identities` migration,
login buttons, and `[auth.oauth2.<provider>]` config stubs. The flow uses
PKCE S256, state validation, OIDC nonce validation, and provider presets for
GitHub, Google, and Microsoft. OAuth support stays behind the `oauth2` feature.

## Signed webhooks

Autumn 0.4.0 added `SignedWebhook` for Stripe, GitHub, Slack, and generic
HMAC callbacks. The extractor verifies the exact raw body bytes, timestamp, and
replay key before handler logic runs.

```rust
#[post("/webhooks/stripe")]
async fn stripe(webhook: SignedWebhook) -> AutumnResult<Json<serde_json::Value>> {
    let event: serde_json::Value = webhook
        .json()
        .map_err(|err| AutumnError::bad_request_msg(format!("invalid JSON: {err}")))?;

    Ok(Json(serde_json::json!({
        "accepted": true,
        "provider": webhook.provider(),
        "delivery_id": webhook.delivery_id(),
        "event_type": webhook.event_type(),
        "event": event,
    })))
}
```

Production replay protection should use Redis:

```toml
[security.webhooks.replay]
backend = "redis"

[security.webhooks.replay.redis]
url = "redis://redis:6379/0"
key_prefix = "myapp:webhooks:replay"
```

Read `docs/guide/signed-webhooks.md` and `examples/signed-webhooks/`.

## Background work

Use built-in jobs and tasks before reaching for a workflow engine:

| Tool | Use for |
|---|---|
| `#[scheduled]` + `.tasks()` | Recurring app-local work; Postgres coordination is available for replicas |
| `#[job]` + `.jobs()` | Request-triggered background work with retries and local/Redis backends |
| `#[task]` + `.one_off_tasks()` | Operator-invoked CLI work via `autumn task` |
| Autumn Harvest | Durable multi-step workflows, activity retries, timers, and dedicated runners |

`autumn-admin-plugin` includes `/admin/jobs` for inspecting, retrying,
discarding, and canceling framework jobs. `GET /actuator/jobs` exposes
lower-level counters.

## File storage and cache plugins

For local or pluggable file storage:

```toml
autumn-web = { version = "0.5", features = ["storage", "multipart"] }
autumn-storage-s3 = "0.5" # when storage.backend = "s3"
```

```rust
let store = autumn_storage_s3::S3BlobStore::from_config(&config.storage.s3)
    .await
    .expect("S3 store");
autumn_web::app().with_blob_store(store).run().await;
```

For shared Redis cache:

```toml
autumn-web = { version = "0.5", features = ["redis"] }
autumn-cache-redis = "0.5"
```

```rust
autumn_web::app()
    .plugin(autumn_cache_redis::RedisCachePlugin::new())
    .run()
    .await;
```

## Configuration

Config layering, lowest to highest:

1. framework defaults
2. profile smart defaults (`dev` / `prod`)
3. `autumn.toml`
4. `[profile.<name>]` inside `autumn.toml`
5. `autumn-{profile}.toml`
6. `AUTUMN_*` environment variables

Profile selection precedence:

1. `AUTUMN_ENV`
2. `AUTUMN_PROFILE`
3. `--profile <name>`
4. debug/release auto-detection

Use `AUTUMN_SECTION__FIELD` for env overrides, for example
`AUTUMN_DATABASE__PRIMARY_URL`, `AUTUMN_JOBS__BACKEND`,
`AUTUMN_SECURITY__SIGNING_SECRET`, and
`AUTUMN_SECURITY__WEBHOOKS__REPLAY__BACKEND`.

## Error handling

JSON errors are standardized as RFC 7807-style Problem Details.
Handlers should return `AutumnResult<T>` and use the typed constructors:

```rust
Err(AutumnError::not_found_msg("post not found"))?;
Err(AutumnError::bad_request_msg("invalid input"))?;
Err(AutumnError::unprocessable_msg("validation failed"))?;
Err(AutumnError::unauthorized_msg("login required"))?;
Err(AutumnError::forbidden_msg("not allowed"))?;
Err(AutumnError::conflict_msg("duplicate delivery"))?;
Err(AutumnError::service_unavailable_msg("queue unavailable"))?;
```

Clients that prefer JSON receive `application/problem+json` with `type`,
`title`, `status`, `detail`, `instance`, `code`, `request_id`, and `errors`.

## CLI

```bash
cargo install autumn-cli --version 0.5.0

autumn new my-app
autumn setup
autumn dev
autumn build
autumn migrate check
autumn migrate --with-maintenance
autumn task --list
autumn task <name> -- --arg value
autumn generate model Post title:String body:Text
autumn generate migration add_posts
autumn generate scaffold Post title:String body:Text --api
autumn generate auth User --oauth github,google --totp --passkeys
autumn generate admin Post
autumn generate mailer UserMailer
autumn generate system-test todo_flow
autumn routes --format json --user-only
autumn doctor --strict --json
autumn config list
autumn flags list
autumn experiments list
autumn maintenance on --message "Migrating database"
autumn webhook sim generic http://localhost:3000/webhooks/test --secret mysecret --payload '{"ok":true}'
autumn dev-loop-bench --dry-run
autumn plugin-check --plugin-name autumn-admin-plugin --prefix /admin
```

`autumn doctor --strict` is the deployment sanity check. It reports unsafe
production defaults, missing primaries, stale replica migrations, missing
signing secrets, and other config problems without printing credentials.

## 0.5.0 release traps

- `AUTUMN_SECURITY__SIGNING_SECRET` is required in `prod` / `production`.
- Use `autumn-storage-s3 = "0.5"` and `autumn-cache-redis = "0.5"`; these
  are companion crates, not `autumn-web` feature names.
- Repository-generated APIs require a policy in production unless
  `security.allow_unauthorized_repository_api = true` is explicit.
- `Mailer::deliver_later` requires a durable queue in production unless
  `mail.allow_in_process_deliver_later_in_production = true` is explicit.
- Signed webhook replay protection should use Redis in multi-replica prod.
- OAuth2/OIDC social-login scaffolding is present. If release notes disagree,
  verify `autumn-cli/src/generate/auth.rs`, `docs/guide/oauth.md`, and current
  branch history before summarizing the release.

## Design invariants

- Postgres only for database-backed apps.
- Diesel + diesel-async only; do not replace core data access with SQLx.
- Stable Rust only.
- Server-rendered HTML first; htmx is the interactivity layer.
- Single binary; external infrastructure is opt-in through config/plugins.
- No GraphQL, DI framework, or deployment tooling in core.
- Primary keys are `i64`; UUIDs are secondary columns only.

## Release and PR workflow

- Base branch is `trunk-dev`, not `trunk`.
- Release tag for this line is `v0.5.0`.
- Published crates are released together at the same workspace version:
  `autumn-macros`, `autumn-web`, `autumn-cli`, `autumn-admin-plugin`,
  `autumn-storage-s3`, and `autumn-cache-redis`.
- The publish gate checks crate metadata, package dry-runs, full docs,
  semver compatibility, release-note alignment, and downstream smoke tests.
- Use `docs/release-checklist.md`, `docs/guide/docs-smoke.md`,
  `CHANGELOG.md`, `RELEASE_NOTES.md`, and `STABILITY.md` for release work.

## Local verification gates

Before pushing an Autumn PR:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p autumn-web --doc --all-features
cargo test -p autumn-cli --test repo_hygiene
```

For docs or generated-app changes, also run the docs smoke procedure in
`docs/guide/docs-smoke.md`. For public API changes, run doctests for the
touched crate so examples compile from an external-consumer perspective.

## Gotchas

- `examples/*/static/css/autumn.css` are generated Tailwind artifacts; ignore
  dirty changes after running examples.
- Proc macros must emit paths through `::autumn_web::...` or
  `::autumn_web::reexports::*`. Do not delegate to upstream macros that emit
  hard-coded transitive dependency paths.
- Workspace builds can hide transitive dependency mistakes. External examples,
  doctests, and downstream smoke tests catch what local `cargo check` misses.
- `CHANGELOG.md` drift between `trunk` and `trunk-dev` can be expected around
  releases; do not propose churn-only back-sync PRs.

## Primary docs

- `README.md`
- `CHANGELOG.md`
- `RELEASE_NOTES.md`
- `STABILITY.md`
- `docs/migrations/0.4.0.md`
- `docs/release-checklist.md`
- `docs/guide/getting-started.md`
- `docs/guide/docs-smoke.md`
- `docs/guide/cloud-native.md`
- `docs/guide/oauth.md`
- `docs/guide/mcp.md`
- `docs/guide/feature-flags.md`
- `docs/guide/experiments.md`
- `docs/guide/runtime-config.md`
- `docs/guide/signed-webhooks.md`
- `docs/guide/storage.md`
- `docs/guide/jobs.md`
- `docs/guide/maintenance-mode.md`
- `docs/guide/dev-loop-latency.md`
- `docs/guide/system-tests.md`
- `docs/guide/testing.md`
- `docs/autumn-workflow-architecture.md`
