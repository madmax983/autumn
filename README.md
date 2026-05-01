# Autumn 🍂

[![CI](https://github.com/madmax983/autumn/actions/workflows/ci.yml/badge.svg)](https://github.com/madmax983/autumn/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/madmax983/autumn/branch/trunk/graph/badge.svg)](https://codecov.io/gh/madmax983/autumn)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Rust: 1.88.0+](https://img.shields.io/badge/rust-1.88.0%2B-orange.svg)](https://www.rust-lang.org)

> Spring Boot-style web framework for Rust, built on [Axum](https://github.com/tokio-rs/axum).

Autumn assembles proven Rust crates into a convention-over-configuration web
stack with proc-macro ergonomics, framework defaults, and customization options when
you need them. If Spring Boot, Rails, or Laravel feels familiar, Autumn aims
for that same "ship the app, not the plumbing" shape in Rust.

## Features

- **Route and app macros** - `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `routes![]`, `#[autumn_web::main]`
- **Pre-rendering pages to static HTML** - `#[static_get]` + `static_routes![]` with `autumn build` pre-rendering to `dist/`
- **Application builder** - `.routes()`, `.tasks()`, `.static_routes()`, `.scoped()`, `.merge()`, and `.nest()`
- **Configuration and profiles** - defaults, `autumn.toml`, `autumn-{profile}.toml`, and `AUTUMN_*` overrides
- **Database ergonomics** - async Postgres pool, `Db` extractor, `#[model]`, `#[repository]`, hooks, and embedded migrations
- **HTML stack** - Maud templating, bundled htmx, Tailwind build pipeline, and static asset serving
- **Operations** - `/health`, `/actuator/*`, structured logging, metrics, and graceful shutdown
- **Background work** - `#[scheduled]` tasks and runtime task visibility at `/actuator/tasks`
- **Companion workflows** - [Autumn Harvest](docs/autumn-workflow-architecture.md) is the separate durable workflow engine for multi-step orchestration when `#[scheduled]` or `#[job]` is not enough
- **Transactional email** - optional `mail` feature with Maud templates, log/file/SMTP transports, and a `Mailer` extractor
- **Security primitives** - session cookies, auth extractor, security headers, CSRF, and `#[secured]`
- **File storage (optional)** - pluggable `BlobStore` trait with built-in `Local` and S3-compatible backends, HMAC-signed URLs, and `MultipartField::save_to_blob_store` (see [storage guide](docs/guide/storage.md))
- **CLI workflow** - `autumn new`, `autumn setup`, `autumn dev`, `autumn build`, and `autumn migrate`

## Quickstart

```bash
# Install the CLI from this workspace
cargo install --path autumn-cli

# Create a new project
autumn new my-app
cd my-app

# Optional: download Tailwind CSS for styled builds
autumn setup

# Optional: scaffold a CRUD resource (see docs/guide/generators.md)
# autumn generate scaffold Post title:String body:Text published:bool

# Development server with file watching
autumn dev

# Or run without watch mode
# cargo run
```

Visit <http://localhost:3000>. Autumn also auto-mounts `/health`,
`/actuator/health`, `/actuator/info`, and `/static/js/htmx.min.js`.

### Watching custom directories

`autumn dev` always watches `src/`, `static/`, `templates/`, and `migrations/`
plus the project's top-level config files (`autumn.toml`, `Cargo.toml`,
`Cargo.lock`, `build.rs`, `tailwind.config.js`). To watch additional folders
(for example, custom view or locale trees), add a `[dev]` section to
`autumn.toml`:

```toml
[dev]
watch_dirs = ["views", "locales"]
```

Listed directories are watched recursively in addition to the defaults.
Multi-segment paths like `content/locales` are supported. Changes inside
them trigger a server restart and a full browser reload. Paths under
`target/` and dotted directories are still ignored.

Entries must be project-relative; absolute paths, `..` traversal,
`target`, and dotted directories (e.g. `.git`) are rejected with a
warning. Missing directories are skipped at startup.

If you add `#[static_get]` routes, `autumn build` pre-renders them into
`dist/`.

## Local-Safe vs Production-Safe

Autumn still distinguishes between "works on your laptop" and "safe to run in a
multi-replica deployment":

- Local-safe defaults: in-memory sessions, pretty logs in `dev`, process-local `#[scheduled]` tasks, and single-binary startup.
- Production-safe defaults: `/live`, `/ready`, `/startup` probes, OTLP telemetry config, Redis-backed sessions, container scaffolding from `autumn new`, and explicit migration jobs before web replicas roll.

If you are deploying beyond a single process, read the
[Cloud-Native Guide](docs/guide/cloud-native.md) before treating the defaults as
done.

## Autumn Harvest

Autumn Harvest is the companion workflow engine for durable, multi-step work:
workflow history, activity retries, timers, singleton orchestration, and
long-running business processes. It is intentionally a separate release train
from `autumn-web`: Harvest can depend on Autumn Web's `AppState` and builder
surface, but Autumn Web's examples and tests should not need Harvest in order to
ship a web release. That keeps the dependency graph pointed in one direction
instead of forming a circular release dependency.

Use built-in `#[scheduled]` tasks and `#[job]` handlers for lightweight app-local
background work. Reach for Harvest when the work needs workflow durability or a
dedicated runner. See the [Harvest architecture notes](docs/autumn-workflow-architecture.md)
for the model and roadmap.

## Example

This is the `main.rs` generated by `autumn new`:

```rust
use autumn_web::prelude::*;

#[get("/")]
async fn index() -> &'static str {
    "Welcome to Autumn!"
}

#[get("/hello/{name}")]
async fn hello_name(name: autumn_web::extract::Path<String>) -> String {
    format!("Hello, {}!", *name)
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index, hello_name])
        .run()
        .await;
}
```

## Built On

- [Axum](https://github.com/tokio-rs/axum) - async HTTP routing and middleware
- [Diesel](https://diesel.rs/) + [diesel-async](https://github.com/weiznich/diesel_async) - database access
- [Maud](https://maud.lambda.xyz/) - compiled HTML templates
- [htmx](https://htmx.org/) - HTML-first interactivity
- [Tailwind CSS](https://tailwindcss.com/) - utility-first styling
- [Tokio](https://tokio.rs/) - async runtime
- [Tracing](https://github.com/tokio-rs/tracing) - structured logging

## Examples

| Example | Description |
|---------|-------------|
| [`examples/hello`](examples/hello) | Minimal hello-world app with route macros and no database |
| [`examples/todo-app`](examples/todo-app) | Classic full-stack CRUD app with Diesel, Maud, htmx, Tailwind, and JSON endpoints |
| [`examples/blog`](examples/blog) | Blog engine with admin UI, validation, and pre-rendering pages to static HTML via `#[static_get]` |
| [`examples/bookmarks`](examples/bookmarks) | Repository macro, generated CRUD API, profiles, scheduled tasks, and actuator endpoints |
| [`examples/wiki`](examples/wiki) | Mutation hooks, revision history, generated REST API, and slug lifecycle management |
| [`examples/reddit-clone`](examples/reddit-clone) | Full-featured Reddit clone using Autumn's server-first stack: auth, sessions, CSRF, `#[secured]`, transactional email, `#[model]`, `#[repository]`, hooks, `#[scheduled]`, `#[job]`, `#[static_get]`, `#[ws]` channels, Redis-capable background jobs and live-feed wakeups, htmx voting, and profiles. It uses built-in jobs instead of Harvest so Autumn Web releases do not depend on the companion workflow crate. |

## Documentation

- [Getting Started Guide](docs/guide/getting-started.md)
- [Code Generators](docs/guide/generators.md) — `autumn generate model | migration | scaffold`
- [Mail Guide](docs/guide/mail.md)
- [Cloud-Native Guide](docs/guide/cloud-native.md)
- [Todo Tutorial](docs/guide/tutorial/index.md)
- [Autumn Harvest Architecture Notes](docs/autumn-workflow-architecture.md)
- [API Reference](https://docs.rs/autumn-web)
- [Pre-rendering Design Notes](docs/design/hybrid-rendering.md)
- [Stability Policy](STABILITY.md) — SemVer, MSRV, and migration commitments

## Stability

Autumn commits to [Semantic Versioning](https://semver.org) for its public
API starting at `1.0.0`. See [STABILITY.md](STABILITY.md) for the full
definition of the stable surface, the MSRV policy, and the migration-guide
process for future major releases.

Until `1.0.0`, Autumn is in its `0.x` series — see the
[pre-1.0 notes](STABILITY.md#pre-10-notes) for what that means in practice.

## Requirements

- Rust 1.88.0+ (edition 2024)
- PostgreSQL for database-backed apps

Autumn can still run without a database if you omit the `[database]` section.

## License

MIT OR Apache-2.0

