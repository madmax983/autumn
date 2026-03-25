# Product Requirements Document: autumn

**Date:** 2026-03-20
**Author:** markm
**Version:** 1.0
**Project Type:** library
**Project Level:** 4
**Status:** Draft

---

## Document Overview

This Product Requirements Document (PRD) defines the functional and non-functional requirements for Autumn, an opinionated convention-over-configuration web application framework for Rust. It serves as the source of truth for what will be built and provides traceability from requirements through implementation.

**Related Documents:**
- Product Brief: `docs/product-brief-autumn-2026-03-20.md`
- Technical Brainstorming: `docs/brainstorming-technical-challenges-2026-03-20.md`
- Competitive Research: `docs/research-competitive-technical-2026-03-20.md`

---

## Executive Summary

Autumn assembles proven Rust crates (Axum, Maud, Tailwind, htmx, Diesel, Postgres) into a Spring Boot-style developer experience with proc-macro-driven conventions and escape hatches at every level. The framework targets experienced web developers adopting Rust who want production-ready web applications without making 30+ infrastructure decisions before their first endpoint.

The competitive landscape (Loco, Cot, Rocket) validates demand for opinionated Rust web frameworks but reveals that no existing solution integrates the full rendering stack (CSS, templating, interactivity). Autumn's Maud+Tailwind+htmx integration is its primary differentiator.

---

## Product Goals

### Business Objectives

1. **Ship v0.1 to crates.io within 6 months** (by September 2026) — a working framework that delivers on the core promise
2. **Achieve ecosystem credibility within 1 year** — recognized as a legitimate option in the Rust web conversation
3. **Become the default recommendation for Rust web applications within 2 years** — v1.0 with stability guarantees and a contributor community

### Success Metrics

**6 months:**
- v0.1 published on crates.io, compiles on stable Rust
- Documented with README, getting started guide, and tutorial
- At least one non-trivial example application

**1 year:**
- 200+ GitHub stars, 1,000+ crates.io downloads
- 10+ issues filed by external users
- At least one conference talk submitted

**2 years:**
- 2,000+ GitHub stars, 20,000+ crates.io downloads
- 5+ active contributors, production use by unknown teams

---

## Functional Requirements

Functional Requirements (FRs) define **what** the system does. Each FR is a specific capability or feature.

Each requirement includes:
- **ID**: Unique identifier (FR-001, FR-002, etc.)
- **Priority**: Must Have / Should Have / Could Have / Won't Have (MoSCoW)
- **Description**: What the system should do
- **Acceptance Criteria**: How to verify it's complete

**Priority guide:**
- **Must Have** = required for v0.1 (6-month ship)
- **Should Have** = required for v1.0 (stability commitment)
- **Could Have** = post-v1.0 consideration

---

### FR-001: CLI Installation

**Priority:** Must Have

**Description:**
`autumn-cli` is installable via `cargo install autumn-cli` and provides the `autumn` command.

**Acceptance Criteria:**
- [ ] `cargo install autumn-cli` succeeds on stable Rust
- [ ] `autumn --version` prints version information
- [ ] `autumn --help` lists available subcommands
- [ ] Binary compiles on Linux, macOS, and Windows

**Dependencies:** None

---

### FR-002: Project Scaffolding

**Priority:** Must Have

**Description:**
`autumn new <name>` generates a complete, compiling, running web application project with database connection, sample routes, styled HTML, and health check.

**Acceptance Criteria:**
- [ ] `autumn new my-app` creates a project directory with the structure defined in the product brief
- [ ] Generated `Cargo.toml` depends on `autumn` crate with correct features
- [ ] Generated `autumn.toml` has sensible defaults (port 3000, localhost DB, info logging)
- [ ] Generated `src/main.rs` contains sample routes using `#[get]` macros
- [ ] Generated `migrations/` contains a sample migration
- [ ] `cd my-app && cargo build` succeeds (assuming Postgres available and Tailwind CLI present)
- [ ] `cargo run` starts a server that serves styled HTML at `http://localhost:3000`

**Dependencies:** FR-001

**Note:** If timeline is tight, this FR can be descoped to a `cargo-generate` template. The framework crate is the product; the CLI is convenience.

---

### FR-003: External Tool Management

**Priority:** Must Have

**Description:**
`autumn new` downloads the Tailwind standalone CLI to `target/autumn/` during project creation. `autumn setup` is available as an explicit command to download/update/verify external tools. `cargo build` never accesses the network.

**Acceptance Criteria:**
- [ ] `autumn new` downloads platform-appropriate Tailwind CLI binary to `target/autumn/tailwindcss`
- [ ] `autumn setup` re-downloads/updates Tailwind CLI with checksum verification
- [ ] `cargo build` uses Tailwind CLI from `target/autumn/` (no network access)
- [ ] If Tailwind CLI not found in `target/autumn/`, `build.rs` checks PATH for `tailwindcss`
- [ ] If neither found, `build.rs` emits `compile_error!("Run `autumn setup` or install tailwindcss")`
- [ ] Platform detection correctly identifies host OS and architecture (not target)
- [ ] Works on Linux x64, Linux arm64, macOS x64, macOS arm64, Windows x64

**Dependencies:** FR-001

---

### FR-004: Workspace Crate Structure

**Priority:** Must Have

**Description:**
Autumn is organized as a Cargo workspace with separate crates for the framework runtime and proc macros, following Rust's requirement that proc macros live in their own crate.

**Acceptance Criteria:**
- [ ] Workspace contains `autumn` (main crate) and `autumn-macros` (proc macro crate)
- [ ] `autumn` re-exports macros so users only need `use autumn_web::prelude::*`
- [ ] `autumn-macros` is a proc-macro crate (`proc-macro = true` in Cargo.toml)
- [ ] Both crates compile on stable Rust (no nightly features)
- [ ] Workspace builds with `cargo build` from root

**Dependencies:** None

---

### FR-005: Route Annotation Macros

**Priority:** Must Have

**Description:**
Proc macros `#[get("/path")]`, `#[post("/path")]`, `#[put("/path")]`, and `#[delete("/path")]` transform annotated async functions into Axum-compatible route handlers. The macro generates a thin wrapper function that calls the user's function unchanged, preserving user-code error locations.

**Acceptance Criteria:**
- [ ] `#[get("/path")]` on an async function generates a valid Axum handler
- [ ] `#[post("/path")]`, `#[put("/path")]`, `#[delete("/path")]` work identically for their HTTP methods
- [ ] Path parameters work: `#[get("/users/{id}")]` with `id: Path<i32>`
- [ ] The macro generates a wrapper function; the user's original function compiles independently
- [ ] Compile errors in user code point at user code, not generated code
- [ ] The macro emits `compile_error!()` for detectable mistakes (missing `async`, non-function items)
- [ ] Each macro generates a route registration struct for use with `routes![]`

**Dependencies:** FR-004

---

### FR-006: Debug Handler Auto-Application

**Priority:** Must Have

**Description:**
Route annotation macros automatically apply `#[axum::debug_handler]` when compiling in debug mode (`cfg(debug_assertions)`), providing improved error messages for handler type mismatches at zero runtime cost.

**Acceptance Criteria:**
- [ ] In debug builds, `#[axum::debug_handler]` is applied to the generated wrapper
- [ ] In release builds, `#[axum::debug_handler]` is not applied (no overhead)
- [ ] Handler type errors produce Axum's improved diagnostic messages in debug mode
- [ ] Missing extractor trait impls (e.g., `FromRequest`) produce actionable error messages

**Dependencies:** FR-005

---

### FR-007: Route Registration Macro

**Priority:** Must Have

**Description:**
A `routes![]` macro (following Rocket's proven pattern) provides explicit, compile-time-validated route registration. Developers list annotated handler functions; the macro expands them into a route collection.

**Acceptance Criteria:**
- [ ] `routes![handler_a, handler_b, handler_c]` expands to a `Vec` of Autumn route structs
- [ ] The macro validates at compile time that listed functions have route annotations
- [ ] Routes from different modules can be listed: `routes![users::list, users::get, posts::list]`
- [ ] Module-level grouping is supported: a function returning `routes![]` per module
- [ ] Typos or non-annotated functions produce a compile error, not a silent omission

**Dependencies:** FR-005

---

### FR-008: Application Entry Point Macro

**Priority:** Must Have

**Description:**
`#[autumn_web::main]` on the application's main function sets up the Tokio runtime, loads configuration, creates the database connection pool, collects routes, and starts the Axum server.

**Acceptance Criteria:**
- [ ] `#[autumn_web::main]` configures the Tokio runtime
- [ ] Loads configuration from `autumn.toml` + environment variables
- [ ] Creates the diesel-async connection pool from config
- [ ] Accepts route collections via `autumn_web::app().routes(...)` builder
- [ ] Binds to configured address and port
- [ ] Logs all mounted routes at startup (method, path, handler name)
- [ ] Panics with a clear error if zero routes are mounted
- [ ] Developer writes minimal bootstrap: `autumn_web::app().routes(my_routes).run().await`

**Dependencies:** FR-005, FR-007, FR-027, FR-030

---

### FR-009: Database Connection Pool

**Priority:** Must Have

**Description:**
Autumn integrates diesel-async with an async connection pool (deadpool-diesel or bb8), created at startup and made available to handlers via Axum's state system.

**Acceptance Criteria:**
- [ ] Connection pool is created from `autumn.toml` database configuration
- [ ] Pool size is configurable (with sensible default, e.g., 10)
- [ ] Pool is stored in Axum's application state
- [ ] Pool health is monitorable (for health check endpoint)
- [ ] Connection acquisition timeout is configurable
- [ ] Pool works with diesel-async's `AsyncPgConnection`

**Dependencies:** FR-008, FR-027

---

### FR-010: Database Extractor

**Priority:** Must Have

**Description:**
A `Db` extractor type allows handlers to declare a database connection need in their function signature. The framework acquires a connection from the pool and provides it to the handler.

**Acceptance Criteria:**
- [ ] Handler with `db: Db` in its signature receives an async database connection
- [ ] Connection is acquired from the pool, not created fresh
- [ ] `?` operator works on Diesel queries using the connection (no turbofish needed)
- [ ] If pool is exhausted, returns an appropriate HTTP error (503)
- [ ] Connection is returned to the pool when the handler completes (including on error)

**Dependencies:** FR-009

---

### FR-011: Model Derive Macro

**Priority:** Must Have

**Description:**
`#[derive(Model)]` on a struct generates Diesel's `Queryable`, `Insertable`, and Serde's `Serialize`/`Deserialize` implementations, reducing boilerplate for data types used across database and API layers.

**Acceptance Criteria:**
- [ ] `#[derive(Model)]` generates `Queryable` and `Insertable` for the struct
- [ ] Also generates `Serialize` and `Deserialize`
- [ ] Table name is inferred from struct name (snake_case pluralized) or overridable via `#[model(table = "custom_name")]`
- [ ] Works with standard Diesel column types (i32, String, NaiveDateTime, etc.)
- [ ] Compile errors from missing Diesel schema point at the struct, not generated code

**Dependencies:** FR-004

---

### FR-012: Path Extractor

**Priority:** Must Have

**Description:**
`Path<T>` extracts typed URL parameters from the request path, re-exported from Axum with Autumn's error handling wired in.

**Acceptance Criteria:**
- [ ] `Path<i32>` extracts a single path parameter as an integer
- [ ] `Path<String>` extracts a single path parameter as a string
- [ ] `Path<(i32, String)>` extracts multiple path parameters as a tuple
- [ ] Type mismatch (e.g., non-numeric string for `Path<i32>`) returns 400 with a clear error
- [ ] Error response format matches handler context (HTML or JSON)

**Dependencies:** FR-005, FR-020

---

### FR-013: Form Extractor

**Priority:** Must Have

**Description:**
`Form<T>` deserializes URL-encoded form data from POST/PUT request bodies, enabling htmx form submissions.

**Acceptance Criteria:**
- [ ] `Form<T>` where `T: Deserialize` extracts form-encoded body data
- [ ] Works with standard HTML form submissions (Content-Type: application/x-www-form-urlencoded)
- [ ] Works with htmx `hx-post` form submissions
- [ ] Validation failures return 422 with a clear error describing which field failed
- [ ] Error response format matches handler context (HTML or JSON)

**Dependencies:** FR-005, FR-020

---

### FR-014: JSON Request Extractor

**Priority:** Must Have

**Description:**
`Json<T>` deserializes JSON request bodies, re-exported from Axum with Autumn's error handling.

**Acceptance Criteria:**
- [ ] `Json<T>` where `T: Deserialize` extracts JSON body data
- [ ] Invalid JSON returns 400 with a clear error message
- [ ] Missing fields return 422 with field-level error details
- [ ] Error responses are always JSON (not HTML) for JSON extractors

**Dependencies:** FR-005, FR-020

---

### FR-015: JSON Response Type

**Priority:** Must Have

**Description:**
Returning `Json<T>` from a handler produces an `application/json` response. The return type is the contract — no configuration or mode-switching needed.

**Acceptance Criteria:**
- [ ] `Json<T>` where `T: Serialize` serializes to JSON with `Content-Type: application/json`
- [ ] A single application can have handlers returning `Markup` (HTML) and `Json<T>` (JSON) side by side
- [ ] Error responses for JSON handlers are JSON-formatted (not HTML error pages)

**Dependencies:** FR-005, FR-020

---

### FR-016: Opaque Error Type

**Priority:** Must Have

**Description:**
`AutumnError` is an opaque error type that wraps any error implementing `std::error::Error`. It automatically converts to an appropriate HTTP response based on the handler's response context.

**Acceptance Criteria:**
- [ ] `AutumnError` implements `std::error::Error`
- [ ] `AutumnError` implements Axum's `IntoResponse`
- [ ] Default HTTP status is 500 Internal Server Error
- [ ] In HTML handlers, renders an error page
- [ ] In JSON handlers, renders a JSON error body (`{"error": "..."}`)
- [ ] Error details are logged but not exposed to clients in production

**Dependencies:** None

---

### FR-017: Blanket Error Conversion

**Priority:** Must Have

**Description:**
A blanket `From<E: std::error::Error> for AutumnError` implementation allows the `?` operator to work in all handlers without manual `From` implementations or turbofish annotations.

**Acceptance Criteria:**
- [ ] `?` works on any `Result<T, E>` where `E: std::error::Error` inside a handler
- [ ] No turbofish annotation needed (e.g., no `.map_err(AutumnError::from)`)
- [ ] Works with Diesel errors, serde errors, std::io errors, and custom error types
- [ ] Compiles on stable Rust without specialization

**Dependencies:** FR-016

---

### FR-018: Custom Error Status Codes

**Priority:** Must Have

**Description:**
An `IntoAutumnError` trait allows developers to opt into custom HTTP status codes for specific error types, overriding the default 500.

**Acceptance Criteria:**
- [ ] `IntoAutumnError` trait has a method returning `StatusCode`
- [ ] Types implementing `IntoAutumnError` produce their specified status code
- [ ] Types not implementing `IntoAutumnError` fall back to 500 via the blanket `From`
- [ ] The proc macro correctly dispatches between `IntoAutumnError` and blanket `From` at compile time
- [ ] Example: `NotFoundError` → 404, `ValidationError` → 422

**Dependencies:** FR-016, FR-017

---

### FR-019: Handler Return Type Contract

**Priority:** Must Have

**Description:**
Handlers declare their response format via return type. `AutumnResult<Markup>` for HTML, `AutumnResult<Json<T>>` for JSON. The error handling system uses this to determine error response format.

**Acceptance Criteria:**
- [ ] `AutumnResult<T>` is a type alias for `Result<T, AutumnError>`
- [ ] Handlers returning `AutumnResult<Markup>` render HTML error pages on error
- [ ] Handlers returning `AutumnResult<Json<T>>` render JSON error bodies on error
- [ ] The explicit return type is required in v0.1 (no silent rewrite)
- [ ] The pattern is recognizable to Spring Boot developers (`ResponseEntity` analog)

**Dependencies:** FR-016, FR-005

---

### FR-020: Maud HTML Integration

**Priority:** Must Have

**Description:**
`Markup` (Maud's return type) is a valid handler return type that produces `text/html` responses. Maud's `html!{}` macro is available via Autumn's prelude.

**Acceptance Criteria:**
- [ ] `use autumn_web::prelude::*` imports Maud's `html!` macro and `Markup` type
- [ ] Returning `Markup` from a handler sends `Content-Type: text/html; charset=utf-8`
- [ ] Maud templates compile at compile time (no runtime template parsing)
- [ ] Tailwind CSS classes used in `html!{}` are picked up by the Tailwind build pipeline
- [ ] Nested `html!{}` calls and component patterns work normally

**Dependencies:** FR-004

---

### FR-021: Tailwind CSS Build Pipeline

**Priority:** Must Have

**Description:**
`build.rs` invokes the locally-present Tailwind CLI to scan Maud templates in `src/**/*.rs` for CSS class names and outputs an optimized CSS file. No network access occurs during build.

**Acceptance Criteria:**
- [ ] `build.rs` finds Tailwind CLI at `target/autumn/tailwindcss` or on PATH
- [ ] Scans `src/**/*.rs` files for Tailwind class names used in Maud templates
- [ ] Outputs tree-shaken CSS to `static/css/autumn.css`
- [ ] Recompiles CSS when source files change (cargo build caching works correctly)
- [ ] Emits clear `compile_error!` if Tailwind CLI is not found
- [ ] Does not access the network during `cargo build`

**Dependencies:** FR-003

---

### FR-022: htmx Integration

**Priority:** Must Have

**Description:**
htmx is embedded as a static asset and served automatically. `hx-*` attributes in Maud templates work without any manual script tag or CDN link.

**Acceptance Criteria:**
- [ ] htmx JavaScript file is embedded in the `autumn` crate (not downloaded at runtime)
- [ ] htmx is served at a known path (e.g., `/static/js/htmx.min.js`)
- [ ] The default HTML layout includes the htmx script tag automatically
- [ ] `hx-get`, `hx-post`, `hx-swap`, `hx-target` attributes work in Maud templates
- [ ] htmx version is pinned and documented

**Dependencies:** FR-023

---

### FR-023: Static Asset Serving

**Priority:** Must Have

**Description:**
Files in the `static/` project directory are served at `/static/` via tower-http's static file serving middleware.

**Acceptance Criteria:**
- [ ] Files placed in `static/` are accessible at `/static/{filename}`
- [ ] Subdirectories work: `static/images/logo.png` → `/static/images/logo.png`
- [ ] Correct MIME types are set based on file extension
- [ ] Static file serving is configured automatically by `#[autumn_web::main]`
- [ ] The generated CSS file (`static/css/autumn.css`) is served correctly

**Dependencies:** FR-008

---

### FR-024: Configuration File

**Priority:** Must Have

**Description:**
`autumn.toml` provides convention-based configuration with sensible defaults for all framework settings. The developer changes behavior by editing config, not rewriting setup code.

**Acceptance Criteria:**
- [ ] `autumn.toml` is loaded from the project root at startup
- [ ] Default values exist for all settings (app runs without any config file)
- [ ] Supported settings include: `server.port`, `server.host`, `database.url`, `database.pool_size`, `log.level`, `log.format`
- [ ] Missing config file uses all defaults (no error)
- [ ] Invalid TOML produces a clear startup error with line/column information
- [ ] Config struct is typed and deserialized at startup (not parsed at runtime per-request)

**Dependencies:** None

---

### FR-025: Environment Variable Overrides

**Priority:** Must Have

**Description:**
Environment variables override `autumn.toml` values using an `AUTUMN_` prefix convention. Environment variables take highest precedence in the three-layer config.

**Acceptance Criteria:**
- [ ] `AUTUMN_SERVER__PORT=8080` overrides `server.port` in config
- [ ] `AUTUMN_DATABASE__URL=postgres://...` overrides `database.url`
- [ ] Double underscore (`__`) maps to TOML nesting (standard convention)
- [ ] Environment variables override file config, which overrides framework defaults
- [ ] `AUTUMN_LOG__LEVEL=debug` overrides log level without touching the file

**Dependencies:** FR-024

---

### FR-026: Framework Configuration Defaults

**Priority:** Must Have

**Description:**
The framework provides sensible defaults for all configuration values, so an Autumn application runs with zero configuration.

**Acceptance Criteria:**
- [ ] Default server port: 3000
- [ ] Default server host: 127.0.0.1
- [ ] Default database URL: `postgres://localhost/autumn_dev` (or from env)
- [ ] Default pool size: 10
- [ ] Default log level: `info`
- [ ] Default log format: pretty-print in dev, JSON when `AUTUMN_ENV=production`
- [ ] Application starts successfully with no `autumn.toml` and no environment variables (using all defaults)

**Dependencies:** FR-024

---

### FR-027: Structured Logging

**Priority:** Must Have

**Description:**
`tracing` and `tracing-subscriber` are pre-configured at startup with request-level spans, request IDs, and environment-appropriate formatting.

**Acceptance Criteria:**
- [ ] Logging is configured automatically by `#[autumn_web::main]`
- [ ] JSON format when profile is production, pretty-print otherwise
- [ ] Every HTTP request gets a unique request ID in its log span
- [ ] Request logs include: method, path, status code, and duration
- [ ] Log level is configurable via `autumn.toml` and `AUTUMN_LOG__LEVEL`
- [ ] Framework startup logs include: port, database URL (masked), mounted routes

**Dependencies:** FR-024, FR-008

---

### FR-028: Health Check Endpoint

**Priority:** Must Have

**Description:**
`GET /health` is mounted automatically and returns service health information including database pool status.

**Acceptance Criteria:**
- [ ] `GET /health` returns 200 OK when the service is healthy
- [ ] Response includes: uptime, database pool status (available/max connections), version
- [ ] Response is JSON formatted
- [ ] If database pool is unhealthy, returns 503 Service Unavailable
- [ ] Health check is mounted without developer configuration
- [ ] Health check path is configurable via `autumn.toml` (default: `/health`)

**Dependencies:** FR-008, FR-009

---

### FR-029: Graceful Shutdown

**Priority:** Must Have

**Description:**
The server handles SIGTERM by draining in-flight connections before exiting, ensuring clean shutdown in container and process manager environments.

**Acceptance Criteria:**
- [ ] SIGTERM signal triggers graceful shutdown
- [ ] In-flight requests are allowed to complete (with configurable timeout, default 30s)
- [ ] New connections are refused during shutdown
- [ ] Database pool is drained after all requests complete
- [ ] Shutdown is logged with duration and pending request count
- [ ] Works on Linux, macOS, and Windows (Ctrl+C on Windows)

**Dependencies:** FR-008

---

### FR-030: Request ID Middleware

**Priority:** Must Have

**Description:**
Every incoming request is assigned a unique ID that flows through logging, error responses, and is available to handlers.

**Acceptance Criteria:**
- [ ] Each request gets a UUID or ULID as a request ID
- [ ] Request ID appears in all log lines for that request
- [ ] Request ID is returned in a response header (`X-Request-Id`)
- [ ] Request ID is available to handlers if needed (via extractor or extension)

**Dependencies:** FR-008

---

### FR-031: Documentation - README

**Priority:** Must Have

**Description:**
The README provides an elevator pitch, quickstart (install → create → run), feature overview, and honest maturity warning.

**Acceptance Criteria:**
- [ ] README includes: one-paragraph description, quickstart (5 steps), feature list, example code
- [ ] Quickstart is copy-pasteable and works end-to-end
- [ ] Includes prominent maturity warning ("v0.1 — experimental, not production-ready")
- [ ] Does not use words like "blazing fast," "production-ready," or "enterprise-grade"
- [ ] Mentions Autumn's relationship to Axum, Diesel, Maud, htmx, Tailwind
- [ ] Links to getting started guide and API docs

**Dependencies:** None

---

### FR-032: Documentation - Getting Started Guide

**Priority:** Must Have

**Description:**
A step-by-step guide taking a developer from zero knowledge to a running Autumn application.

**Acceptance Criteria:**
- [ ] Covers: prerequisites, installation, project creation, first route, database setup, running
- [ ] Includes deployment section (single binary + config)
- [ ] A developer with no Autumn experience can follow it end-to-end without reading source code
- [ ] Tested by someone other than the author (or tested from a clean environment)

**Dependencies:** FR-031

---

### FR-033: Documentation - Tutorial

**Priority:** Must Have

**Description:**
A tutorial building a non-trivial application (e.g., a todo app) that demonstrates all core features working together.

**Acceptance Criteria:**
- [ ] Demonstrates: routes, database queries, Maud templates, Tailwind styling, htmx interactivity, form submissions, JSON API, error handling, configuration
- [ ] Builds incrementally (each section adds a feature)
- [ ] Final result is a complete, styled, interactive CRUD application
- [ ] Code samples are tested and match the example application in the repo

**Dependencies:** FR-032

---

### FR-034: Example Application

**Priority:** Must Have

**Description:**
A non-trivial example application in the repository demonstrating all core features in a realistic context.

**Acceptance Criteria:**
- [ ] Multiple route modules (at least 2 resource types)
- [ ] Database queries (list, get by ID, create)
- [ ] Maud templates with Tailwind classes and htmx attributes
- [ ] Form submissions that write to the database
- [ ] JSON API endpoints for the same resources
- [ ] Error handling (404 for missing resources, validation errors)
- [ ] Configuration override via `autumn.toml`
- [ ] Compiles and runs as a standalone application

**Dependencies:** All Must Have FRs

---

### FR-035: API Documentation

**Priority:** Must Have

**Description:**
All public types, traits, and macros have `cargo doc` documentation with examples.

**Acceptance Criteria:**
- [ ] `cargo doc --open` produces navigable API documentation
- [ ] All public types have doc comments explaining purpose and usage
- [ ] Key types (`Db`, `AutumnError`, `AutumnResult`) have code examples in their docs
- [ ] Macros (`#[get]`, `routes![]`, `#[autumn_web::main]`) have usage examples
- [ ] No `missing_docs` warnings on public items

**Dependencies:** FR-004

---

### FR-036: Cross-Platform CI

**Priority:** Must Have

**Description:**
GitHub Actions CI pipeline tests Autumn on Linux, macOS, and Windows against stable Rust.

**Acceptance Criteria:**
- [ ] CI runs on: ubuntu-latest, macos-latest, windows-latest
- [ ] Tests against stable Rust (minimum supported version pinned in CI and documented)
- [ ] Pipeline includes: `cargo fmt --check`, `cargo clippy`, `cargo test`
- [ ] CI passes before any release to crates.io
- [ ] Postgres service container (or equivalent) available for database tests

**Dependencies:** None

---

### FR-037: crates.io Publication

**Priority:** Must Have

**Description:**
`autumn` and `autumn-macros` crates are published to crates.io and installable via `cargo add autumn`.

**Acceptance Criteria:**
- [ ] `cargo add autumn` adds the framework to a project's dependencies
- [ ] `autumn-macros` is published as a dependency of `autumn` (users don't interact with it directly)
- [ ] `autumn-cli` is published separately and installable via `cargo install autumn-cli`
- [ ] Crate metadata (description, license, repository URL, keywords) is complete
- [ ] License is MIT or Apache-2.0 (dual-licensed)

**Dependencies:** FR-036

---

### FR-038: Cargo Feature Flags

**Priority:** Must Have

**Description:**
Cargo features allow opting out of framework subsystems. The default feature set includes everything; features can be disabled individually.

**Acceptance Criteria:**
- [ ] `default` features include full stack (Maud, Tailwind, htmx, Diesel)
- [ ] `default-features = false` disables the rendering stack
- [ ] Individual features: `tailwind`, `htmx`, `maud` can be toggled
- [ ] Disabling `tailwind` removes the build.rs Tailwind pipeline
- [ ] Feature combinations compile correctly (no orphaned dependencies)

**Dependencies:** FR-004

---

### FR-039: CORS Configuration

**Priority:** Should Have

**Description:**
CORS is configurable via `autumn.toml` with sensible defaults: permissive in development, locked down in production.

**Acceptance Criteria:**
- [ ] CORS middleware is applied automatically
- [ ] Dev default: allow all origins
- [ ] Production default: no CORS (same-origin only)
- [ ] Configurable via `autumn.toml`: allowed origins, methods, headers
- [ ] `AUTUMN_CORS__ALLOWED_ORIGINS` env var override works

**Dependencies:** FR-024

---

### FR-040: Custom Middleware

**Priority:** Should Have

**Description:**
Developers can add custom Tower middleware to the Autumn application via a builder method.

**Acceptance Criteria:**
- [ ] `autumn_web::app().layer(my_middleware)` adds Tower middleware
- [ ] Standard Tower middleware (compression, timeout, rate limiting) works
- [ ] Middleware ordering is predictable and documented
- [ ] Custom middleware receives the request ID from FR-030

**Dependencies:** FR-008

---

### FR-041: Raw Axum Route Mounting

**Priority:** Should Have

**Description:**
Raw Axum routers can be mounted alongside Autumn's annotated routes via `.merge()`, providing an escape hatch for advanced routing needs.

**Acceptance Criteria:**
- [ ] `autumn_web::app().merge(axum_router)` mounts raw Axum routes
- [ ] Axum routes have access to the same shared state (database pool, config)
- [ ] Autumn middleware (logging, request ID) applies to merged routes
- [ ] Documentation explains when and how to use this escape hatch

**Dependencies:** FR-008

---

### FR-042: Trait-Based Subsystem Replacement

**Priority:** Should Have

**Description:**
Key framework subsystems (database pool, config loader) are abstracted behind traits, allowing advanced users to replace them with custom implementations.

**Acceptance Criteria:**
- [ ] `DatabasePoolProvider` trait defines how to create and manage a connection pool
- [ ] `ConfigLoader` trait defines how to load configuration
- [ ] Default implementations use Diesel and TOML; custom implementations can be substituted
- [ ] Trait definitions are stable and documented
- [ ] At least one example shows replacing a subsystem

**Dependencies:** FR-008

---

### FR-043: Dev/Prod Profiles

**Priority:** Should Have

**Description:**
`autumn.toml` supports `[profile.dev]` and `[profile.prod]` sections for environment-specific configuration.

**Acceptance Criteria:**
- [ ] Profile is selected via `AUTUMN_ENV` environment variable (default: `development`)
- [ ] `[profile.dev]` settings override base settings in development
- [ ] `[profile.prod]` settings override base settings in production
- [ ] Log format automatically switches (pretty in dev, JSON in prod)
- [ ] Environment variables still override profile settings

**Dependencies:** FR-024

---

### FR-044: Migration Management

**Priority:** Should Have

**Description:**
Pending database migrations are auto-run on startup in development mode. Production requires explicit migration via CLI.

**Acceptance Criteria:**
- [ ] In dev mode, pending Diesel migrations run automatically on server startup
- [ ] In production mode, migrations are not auto-run (opt-in via config flag)
- [ ] `autumn migrate` CLI command runs migrations explicitly
- [ ] Failed migrations produce clear error messages with the failing SQL
- [ ] Migration status is logged at startup

**Dependencies:** FR-009, FR-043

---

### FR-045: Error Page Overrides

**Priority:** Should Have

**Description:**
Default Maud-rendered error pages (404, 500) are provided, with a mechanism for developers to override them with custom templates.

**Acceptance Criteria:**
- [ ] Default 404 page is styled with Tailwind and includes the request path
- [ ] Default 500 page is styled and includes the request ID (not the error details)
- [ ] Developers can override error pages by implementing a trait or providing template functions
- [ ] JSON handlers never receive HTML error pages (always JSON errors)

**Dependencies:** FR-020, FR-016

---

### FR-046: CSRF Protection

**Priority:** Should Have

**Description:**
CSRF protection for form submissions prevents cross-site request forgery attacks on state-changing endpoints.

**Acceptance Criteria:**
- [ ] CSRF tokens are generated and validated for POST/PUT/DELETE form submissions
- [ ] A Maud helper renders the CSRF token as a hidden form field
- [ ] Token validation failure returns 403 Forbidden with a clear message
- [ ] CSRF protection is on by default for form handlers, off for JSON API handlers
- [ ] Configurable via `autumn.toml` (disable for specific routes or globally)

**Dependencies:** FR-013, FR-020

---

### FR-047: Secure Headers

**Priority:** Should Have

**Description:**
Security-related HTTP headers are set by default on all responses.

**Acceptance Criteria:**
- [ ] `X-Content-Type-Options: nosniff` on all responses
- [ ] `X-Frame-Options: DENY` by default (configurable)
- [ ] `Strict-Transport-Security` in production mode
- [ ] `Content-Security-Policy` with sensible defaults for htmx
- [ ] Headers are configurable via `autumn.toml`

**Dependencies:** FR-008

---

### FR-048: Semver Stability Guarantee

**Priority:** Should Have

**Description:**
v1.0 release carries a semver commitment: no breaking changes without a major version bump.

**Acceptance Criteria:**
- [ ] Public API surface is documented and intentional
- [ ] Breaking changes require a major version bump (1.x → 2.0)
- [ ] `#[doc(hidden)]` or `#[non_exhaustive]` used appropriately for internal types
- [ ] Migration guide provided for any breaking changes
- [ ] MSRV (Minimum Supported Rust Version) policy is documented

**Dependencies:** All Must Have FRs

---

## Non-Functional Requirements

Non-Functional Requirements (NFRs) define **how** the system performs - quality attributes and constraints.

---

### NFR-001: Performance - Framework Overhead

**Priority:** Must Have

**Description:**
Autumn's framework overhead (routing, middleware, extraction) adds negligible latency compared to raw Axum. The framework should not be the bottleneck.

**Acceptance Criteria:**
- [ ] Framework overhead per request < 1ms (measured as difference between Autumn handler and equivalent raw Axum handler)
- [ ] No runtime reflection or dynamic dispatch in the hot path
- [ ] Proc macros generate static dispatch code (no `Box<dyn>` in route handling)
- [ ] Benchmark suite exists comparing Autumn vs raw Axum for equivalent operations

**Rationale:**
Developers choosing Rust for web already care about performance. If Autumn adds meaningful overhead, they'll use raw Axum instead.

---

### NFR-002: Compilation - Build Time

**Priority:** Must Have

**Description:**
Autumn should not significantly increase build times compared to depending on its constituent crates directly.

**Acceptance Criteria:**
- [ ] Clean build of a simple Autumn application < 90 seconds on a modern machine
- [ ] Incremental build after a single-file change < 15 seconds
- [ ] Proc macro expansion is fast (< 100ms for a project with 50 routes)
- [ ] Build time is tracked in CI and regressions are caught

**Rationale:**
Rust's build times are already a pain point. A framework that makes them significantly worse will be abandoned.

---

### NFR-003: Compatibility - Stable Rust

**Priority:** Must Have

**Description:**
Autumn compiles on stable Rust with no nightly features required. The minimum supported Rust version (MSRV) is pinned and documented.

**Acceptance Criteria:**
- [ ] `cargo build` succeeds on the latest stable Rust release
- [ ] No `#![feature(...)]` in any crate
- [ ] MSRV is documented in README and Cargo.toml (`rust-version` field)
- [ ] CI tests against MSRV in addition to latest stable
- [ ] MSRV is no older than 6 months (trailing stable releases)

**Rationale:**
Requiring nightly is a non-starter for production adoption. This is stated as a hard constraint in the product brief.

---

### NFR-004: Compatibility - Cross-Platform

**Priority:** Must Have

**Description:**
Autumn applications compile and run on Linux, macOS, and Windows.

**Acceptance Criteria:**
- [ ] CI tests pass on ubuntu-latest, macos-latest, windows-latest
- [ ] Tailwind CLI binary management handles all three platforms
- [ ] File paths, process signals, and networking work correctly on all platforms
- [ ] Documentation does not assume Linux (commands work on all platforms)

**Rationale:**
Developers build on macOS and Windows. Deploy on Linux. All three must work.

---

### NFR-005: Build Determinism - No Network in cargo build

**Priority:** Must Have

**Description:**
`cargo build` never accesses the network. All external tool dependencies are managed outside the build process.

**Acceptance Criteria:**
- [ ] `build.rs` does not make HTTP requests
- [ ] `build.rs` does not download files
- [ ] Offline builds succeed if Tailwind CLI is pre-installed
- [ ] CI builds work without special network configuration
- [ ] Compatible with Nix, Bazel, and other hermetic build systems

**Rationale:**
Network access in `build.rs` breaks CI, reproducible builds, offline development, and corporate environments. Research confirmed this is a critical requirement.

---

### NFR-006: Error Quality - Actionable Diagnostics

**Priority:** Must Have

**Description:**
All framework-generated errors (compile-time and runtime) include actionable messages that point at user code, not generated code.

**Acceptance Criteria:**
- [ ] Proc macro errors use `compile_error!()` with human-readable messages for common mistakes
- [ ] `#[axum::debug_handler]` is auto-applied in debug builds for handler type errors
- [ ] Runtime errors include the request ID and enough context to diagnose
- [ ] Error messages suggest fixes (e.g., "Did you forget `#[derive(Deserialize)]` on your form struct?")
- [ ] No error message ever points at macro-generated code as the primary location

**Rationale:**
Research found that Cot was criticized for poor error messages. Axum's `debug_handler` exists specifically because handler errors are a known pain point. Autumn must do better than the status quo, not worse.

---

### NFR-007: Maintainability - Code Quality

**Priority:** Must Have

**Description:**
The framework codebase follows Rust best practices and the project's coding standards (from CLAUDE.md).

**Acceptance Criteria:**
- [ ] `cargo fmt` produces no changes (enforced in CI)
- [ ] `cargo clippy` with pedantic lints produces no warnings (enforced in CI)
- [ ] No `unwrap()` in library code (documented exceptions allowed)
- [ ] `thiserror` for library error types
- [ ] Test coverage > 85% for library crates
- [ ] Doc comments on all public items

**Rationale:**
A single-developer project must have high code quality to remain maintainable. Future contributors need readable, well-tested code.

---

### NFR-008: Maintainability - No Upstream Forks

**Priority:** Must Have

**Description:**
Autumn must not fork any upstream crate. Dependencies are used as-is, with workarounds or upstream contributions for any gaps.

**Acceptance Criteria:**
- [ ] No forked dependencies in Cargo.toml (no `git = "..."` pointing to personal forks)
- [ ] All dependencies are published on crates.io
- [ ] Any upstream bugs are worked around or reported with PRs (not forked)

**Rationale:**
Forking creates unmaintainable divergence that a single developer cannot sustain. Stated as a hard constraint in the product brief.

---

### NFR-009: Security - Dependency Auditing

**Priority:** Should Have

**Description:**
Dependencies are audited for known vulnerabilities.

**Acceptance Criteria:**
- [ ] `cargo audit` runs in CI and fails on known vulnerabilities
- [ ] Dependencies are kept reasonably up to date (no dependencies > 2 major versions behind)
- [ ] Security advisories for upstream crates are monitored

**Rationale:**
A web framework is a high-value attack surface. Known vulnerabilities in dependencies are the lowest-hanging security fruit.

---

### NFR-010: Usability - Time To First Endpoint

**Priority:** Must Have

**Description:**
A developer with Rust and Postgres installed can go from zero to a running Autumn application with their first custom endpoint in under 5 minutes.

**Acceptance Criteria:**
- [ ] Timed test: `cargo install autumn-cli && autumn new my-app && cd my-app && cargo run` completes in < 3 minutes (including first compile)
- [ ] Adding a new route requires editing one file and recompiling (< 15 seconds incremental)
- [ ] No configuration changes needed for the happy path
- [ ] The first page the developer sees is styled (not unstyled HTML)

**Rationale:**
This is the framework's core value proposition. If the first 5 minutes aren't fast and delightful, the rest doesn't matter.

---

### NFR-011: Documentation - Completeness

**Priority:** Must Have

**Description:**
Documentation is sufficient for a developer to use all framework features without reading source code.

**Acceptance Criteria:**
- [ ] Every public API has doc comments with examples
- [ ] Getting started guide covers end-to-end workflow
- [ ] At least one tutorial demonstrates a realistic application
- [ ] Error messages reference documentation where applicable
- [ ] Documentation is tested (code examples compile)

**Rationale:**
The product brief identifies documentation as a top priority. A framework without docs is a library with opinions.

---

### NFR-012: Licensing

**Priority:** Must Have

**Description:**
Autumn is dual-licensed under MIT and Apache-2.0, the standard Rust ecosystem licenses.

**Acceptance Criteria:**
- [ ] LICENSE-MIT and LICENSE-APACHE files exist in the repository
- [ ] Cargo.toml `license` field is set to `MIT OR Apache-2.0`
- [ ] All source files have SPDX headers or the license is documented in the repo root
- [ ] No dependencies with incompatible licenses (GPL, AGPL, SSPL)

**Rationale:**
MIT/Apache-2.0 dual licensing is the Rust ecosystem standard. Incompatible licenses block corporate adoption.

---

## Epics

Epics are logical groupings of related functionality that will be broken down into user stories during sprint planning (Phase 4).

Each epic maps to multiple functional requirements and will generate 2-10 stories.

---

### EPIC-001: Project Scaffolding & CLI

**Description:**
The developer tooling that creates new Autumn projects and manages external dependencies (Tailwind CLI). This is the "first 60 seconds" experience.

**Functional Requirements:**
- FR-001: CLI Installation
- FR-002: Project Scaffolding
- FR-003: External Tool Management

**Story Count Estimate:** 5-7

**Priority:** Must Have

**Business Value:**
The `autumn new` experience is the single most important adoption moment. If it doesn't work flawlessly, developers never see the rest of the framework. This is the `spring init` / `rails new` equivalent.

---

### EPIC-002: Route System

**Description:**
The proc macro system that transforms annotated functions into Axum handlers and the `routes![]` macro that collects them into a router. This is the core DX innovation.

**Functional Requirements:**
- FR-005: Route Annotation Macros
- FR-006: Debug Handler Auto-Application
- FR-007: Route Registration Macro
- FR-012: Path Extractor

**Story Count Estimate:** 8-10

**Priority:** Must Have

**Business Value:**
`#[get("/users")]` is the framework's signature feature — the thing a Spring Boot developer recognizes immediately. The route system is the load-bearing wall; everything else is built on top of it. This is also the highest-risk epic (proc macros are hard).

---

### EPIC-003: Application Bootstrap

**Description:**
The `#[autumn_web::main]` macro and application builder that wires together configuration, database, routes, and server startup into a single entry point.

**Functional Requirements:**
- FR-008: Application Entry Point Macro
- FR-030: Request ID Middleware

**Story Count Estimate:** 4-6

**Priority:** Must Have

**Business Value:**
The entry point macro is what makes Autumn a framework rather than a collection of macros. `autumn_web::app().routes(my_routes).run().await` is the entire bootstrap — everything else is automatic.

---

### EPIC-004: Database Layer

**Description:**
Diesel-async connection pool integration, the `Db` extractor, and the `#[derive(Model)]` convenience macro.

**Functional Requirements:**
- FR-009: Database Connection Pool
- FR-010: Database Extractor
- FR-011: Model Derive Macro

**Story Count Estimate:** 6-8

**Priority:** Must Have

**Business Value:**
A web framework without database integration is just a router. The `Db` extractor and `#[derive(Model)]` are what make Autumn a full-stack framework. Diesel's compile-time query checking is a key differentiator over Loco's SeaORM.

---

### EPIC-005: Error Handling

**Description:**
The `AutumnError` type, blanket conversions, `IntoAutumnError` trait, and context-aware error responses (HTML vs JSON).

**Functional Requirements:**
- FR-016: Opaque Error Type
- FR-017: Blanket Error Conversion
- FR-018: Custom Error Status Codes
- FR-019: Handler Return Type Contract

**Story Count Estimate:** 5-7

**Priority:** Must Have

**Business Value:**
The `?` operator working everywhere without ceremony is a defining feature. Error handling is where most Rust web apps accumulate boilerplate; Autumn's job is to eliminate it while keeping errors informative and context-aware.

---

### EPIC-006: Rendering Stack

**Description:**
Maud integration, Tailwind CSS build pipeline, htmx bundling — the full-stack frontend experience that no competitor offers.

**Functional Requirements:**
- FR-020: Maud HTML Integration
- FR-021: Tailwind CSS Build Pipeline
- FR-022: htmx Integration
- FR-013: Form Extractor

**Story Count Estimate:** 6-8

**Priority:** Must Have

**Business Value:**
This is Autumn's moat. No existing Rust framework integrates CSS, templating, and interactivity. The first `cargo run` producing styled, interactive HTML is the "aha" moment that makes developers stay. Research confirmed this is the primary competitive differentiator.

---

### EPIC-007: Configuration & Defaults

**Description:**
The three-layer configuration system (defaults → TOML → env vars) and production defaults (logging, health check, graceful shutdown).

**Functional Requirements:**
- FR-024: Configuration File
- FR-025: Environment Variable Overrides
- FR-026: Framework Configuration Defaults
- FR-027: Structured Logging
- FR-028: Health Check Endpoint
- FR-029: Graceful Shutdown

**Story Count Estimate:** 6-8

**Priority:** Must Have

**Business Value:**
Production defaults are what separate a framework from a demo. Logging, health checks, and graceful shutdown are the things every production app needs and every developer forgets until the first deployment. Having them on by default is a core part of the "Spring Boot experience."

---

### EPIC-008: JSON & Static Assets

**Description:**
JSON request/response handling and static file serving — the API escape hatch and asset pipeline.

**Functional Requirements:**
- FR-014: JSON Request Extractor
- FR-015: JSON Response Type
- FR-023: Static Asset Serving

**Story Count Estimate:** 3-4

**Priority:** Must Have

**Business Value:**
The "return type is the contract" design (Markup → HTML, Json<T> → JSON) is elegant and recognizable to Spring Boot developers (ResponseEntity pattern). Static asset serving completes the full-stack story.

---

### EPIC-009: Documentation & Examples

**Description:**
All user-facing documentation: README, getting started guide, tutorial, example application, and API docs.

**Functional Requirements:**
- FR-031: Documentation - README
- FR-032: Documentation - Getting Started Guide
- FR-033: Documentation - Tutorial
- FR-034: Example Application
- FR-035: API Documentation

**Story Count Estimate:** 7-9

**Priority:** Must Have

**Business Value:**
Research showed that community credibility depends on documentation quality. Cot was criticized for sparse docs. The README is often the only chance to convert a visitor into a user. The example application is both documentation and integration test.

---

### EPIC-010: CI & Distribution

**Description:**
Cross-platform CI pipeline, crates.io publication, and feature flag system.

**Functional Requirements:**
- FR-036: Cross-Platform CI
- FR-037: crates.io Publication
- FR-038: Cargo Feature Flags

**Story Count Estimate:** 4-5

**Priority:** Must Have

**Business Value:**
`cargo add autumn` is the entry point. If it doesn't work, nothing else matters. Feature flags enable the opt-out flexibility that prevents the "framework lock-in" objection.

---

### EPIC-011: v1.0 Security & Stability

**Description:**
The features required to call Autumn "production-ready" and commit to API stability: CORS, CSRF, secure headers, middleware, escape hatches, migration management, and semver guarantee.

**Functional Requirements:**
- FR-039: CORS Configuration
- FR-040: Custom Middleware
- FR-041: Raw Axum Route Mounting
- FR-042: Trait-Based Subsystem Replacement
- FR-043: Dev/Prod Profiles
- FR-044: Migration Management
- FR-045: Error Page Overrides
- FR-046: CSRF Protection
- FR-047: Secure Headers
- FR-048: Semver Stability Guarantee

**Story Count Estimate:** 10-14

**Priority:** Should Have

**Business Value:**
These features are the difference between "interesting framework" and "framework I'd bet my business on." The semver guarantee is the culmination — the promise that Autumn is stable enough to invest in.

---

## User Stories (High-Level)

Detailed user stories will be created during sprint planning (Phase 4). Below are representative stories per epic.

**EPIC-001: Project Scaffolding**
- As a web developer evaluating Rust, I want to run one command and have a working web application so that I can evaluate the framework without spending an hour on setup.
- As a developer starting a new project, I want the scaffolded app to have styled HTML and a database connection so that I'm building on a real foundation, not a skeleton.

**EPIC-002: Route System**
- As a Spring Boot developer, I want to annotate a function with `#[get("/users")]` and have it become a route so that I can use the pattern I already know.
- As a developer debugging a type error, I want the compiler error to point at my code, not at macro-generated code, so that I can fix it without `cargo expand`.

**EPIC-003: Application Bootstrap**
- As a developer, I want to write `autumn_web::app().routes(my_routes).run().await` and have everything boot so that I don't write 150 lines of setup in main.rs.

**EPIC-004: Database Layer**
- As a developer, I want to declare `db: Db` in my handler and get a database connection so that I never manually manage connection pools.
- As a Rust developer, I want my database queries type-checked at compile time so that query bugs are caught before runtime.

**EPIC-005: Error Handling**
- As a developer, I want `?` to work in every handler without turbofish so that error handling is invisible when I don't need to customize it.
- As a developer, I want errors in HTML handlers to render error pages and errors in JSON handlers to render JSON so that error responses match the context.

**EPIC-006: Rendering Stack**
- As a developer, I want my first `cargo run` to produce styled, interactive HTML so that Autumn feels like a real full-stack framework, not a router.
- As a developer using htmx, I want `hx-post` form submissions to just work without any script tags or CDN links.

**EPIC-007: Configuration & Defaults**
- As a developer deploying to production, I want logging, health checks, and graceful shutdown to be on by default so that I don't forget them.
- As a developer, I want to override any config value via environment variable so that I can configure my app without rebuilding.

**EPIC-008: JSON & Static Assets**
- As a developer, I want to return `Json<T>` from one handler and `Markup` from another in the same app so that I can serve both a web UI and an API.

**EPIC-009: Documentation**
- As a developer who has never used Autumn, I want to go from "what is this?" to a running app by reading the README and getting started guide.

---

## User Personas

### Primary: The Framework Developer

Experienced web developer (3-10 years), coming from Spring Boot, Rails, Django, or Laravel. Knows what convention-over-configuration means. Choosing Rust for performance/safety. Building startup MVPs, internal tools, SaaS products, CRUD applications. Their current pain: tried Rust for web once and went back because of integration overhead.

### Secondary: The Rust Systems Developer

Knows Rust well (CLIs, libraries, infrastructure) but has never built a web app in it. Needs a web frontend for something that exists as a CLI or library. Wants the framework to handle the parts they don't care about (assets, CORS, health checks) so they can focus on business logic.

### Not The Target (Yet)

- Junior Rust developers (proc macros hide too much complexity for learning)
- Microservices-only teams (Autumn's full-stack opinions are overhead)
- SPA backend teams (Autumn's opinion is server-rendered HTML)

---

## User Flows

### Flow 1: New Project Creation (Primary)

```
1. Install CLI: cargo install autumn-cli
2. Create project: autumn new my-app
3. Start database: docker run -d -p 5432:5432 postgres
4. Build and run: cd my-app && cargo run
5. Visit: http://localhost:3000 → styled page with sample form
6. Add route: edit src/main.rs, add #[get("/hello/{name}")]
7. Rebuild: cargo run
8. Visit: http://localhost:3000/hello/world → it works
```

### Flow 2: Adding a Database-Backed Feature

```
1. Create migration: diesel migration generate create_users
2. Write SQL in up.sql/down.sql
3. Run migration: diesel migration run
4. Define model: #[derive(Model)] struct User { ... }
5. Write handler: #[get("/users")] async fn list(db: Db) -> AutumnResult<Markup> { ... }
6. Add to routes: routes![list_users]
7. Rebuild and visit: list of users rendered with Tailwind styling
```

### Flow 3: Deploying to Production

```
1. Build: cargo build --release
2. Copy binary + autumn.toml to server
3. Set: AUTUMN_DATABASE__URL=postgres://prod-server/myapp
4. Set: AUTUMN_ENV=production
5. Run binary
6. Verify: GET /health returns 200 with pool status
```

---

## Dependencies

### Internal Dependencies

- `autumn` crate depends on `autumn-macros` (proc macro crate)
- `autumn-cli` depends on `autumn` (for config schema validation)
- Example application depends on `autumn` (integration test)

### External Dependencies

| Crate | Purpose | Version Strategy |
|-------|---------|-----------------|
| axum | HTTP framework | Track latest stable |
| tokio | Async runtime | Track latest stable |
| diesel + diesel-async | ORM + async adapter | Track latest stable |
| deadpool-diesel | Connection pool | Track latest stable |
| maud | HTML templates | Track latest stable |
| tower-http | Static files, CORS | Track latest stable |
| tracing + tracing-subscriber | Logging | Track latest stable |
| serde + toml | Configuration | Track latest stable |
| thiserror | Error types | Track latest stable |
| syn + quote + proc-macro2 | Proc macro impl | Track latest stable |

| External Tool | Purpose | Version Strategy |
|---------------|---------|-----------------|
| Tailwind CSS CLI | CSS compilation | Pin specific version, update deliberately |
| htmx | Client-side interactivity | Embed specific version in crate |
| PostgreSQL | Database | Document minimum supported version (14+) |

---

## Assumptions

1. **Axum remains the Rust HTTP framework winner** (Confidence: High)
2. **diesel-async is production-ready** (Confidence: Medium-High)
3. **Server-rendered HTML + htmx is a durable architecture** (Confidence: Medium-High)
4. **Tailwind standalone CLI remains available** (Confidence: High)
5. **Rust developers want an opinionated web framework** (Confidence: Medium — the deepest assumption)
6. **One developer can build a credible framework** (Confidence: Medium — 3-month gut check)
7. **`routes![]` explicit registration is acceptable DX** (Confidence: High — Rocket has proven this for years)

---

## Out of Scope

- **ORM abstraction** — Autumn uses Diesel, not an abstraction over ORMs
- **Frontend framework** — No WASM, client-side routing, or virtual DOM
- **Multiple databases** — Postgres only
- **GraphQL** — API escape hatch is REST/JSON only
- **Deployment tooling** — No Docker generation, K8s manifests, or cloud integrations
- **Dependency injection** — Rust's type system already solves this
- **Runtime plugin loading** — Extensions happen at compile time
- **Linker-based route auto-discovery** — Confirmed cross-crate bug (rust-lang/rust#67209) makes this unsuitable for v0.1; may revisit if compiler bug is fixed

---

## Open Questions

1. **`AutumnResult<T>` vs bare return types:** v0.1 requires explicit `AutumnResult<T>`. Should v0.2 add silent return type rewriting? Depends on whether error messages are good enough.
2. **Connection pool crate:** deadpool-diesel vs bb8 vs custom. Needs a spike to determine best async Diesel integration.
3. **Tailwind v4 standalone CLI:** Does v4's standalone CLI exist and work with Maud template scanning? Needs verification before committing the build pipeline.
4. **`#[derive(Model)]` scope:** Should it handle relations (belongs_to, has_many) or stay limited to single-table mappings for v0.1?
5. **htmx version:** Pin to htmx 1.x (stable) or htmx 2.x (newer, some breaking changes)?
6. **Middleware ordering:** How does Autumn's automatic middleware (logging, request ID, CORS) interact with user-added middleware? Needs explicit ordering documentation.

---

## Approval & Sign-off

### Stakeholders

- **Mark (Creator/Sole Maintainer)** — High influence. Final decision maker.
- **Upstream crate maintainers** — Informed stakeholders (no approval needed).
- **Rust web community** — Validation via adoption, not formal sign-off.

### Approval Status

- [x] Product Owner (Mark)
- [ ] Engineering Lead (Mark — pending architecture review)

---

## Revision History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-03-20 | markm | Initial PRD informed by product brief, brainstorming, and competitive research |

---

## Next Steps

### Phase 3: Architecture

Run `/bmad:architecture` to create system architecture based on these requirements.

The architecture will address:
- All functional requirements (FRs)
- All non-functional requirements (NFRs)
- Crate structure and dependency graph
- Proc macro design and code generation strategy
- Configuration system design
- Error handling type hierarchy
- Build pipeline architecture

### Phase 4: Sprint Planning

After architecture is complete, run `/bmad:sprint-planning` to:
- Break epics into detailed user stories
- Estimate story complexity
- Plan sprint iterations
- Begin implementation

---

**This document was created using BMAD Method v6 - Phase 2 (Planning)**

*To continue: Run `/bmad:workflow-status` to see your progress and next recommended workflow.*

---

## Appendix A: Requirements Traceability Matrix

| Epic ID | Epic Name | Functional Requirements | Story Count (Est.) |
|---------|-----------|-------------------------|-------------------|
| EPIC-001 | Project Scaffolding & CLI | FR-001, FR-002, FR-003 | 5-7 |
| EPIC-002 | Route System | FR-005, FR-006, FR-007, FR-012 | 8-10 |
| EPIC-003 | Application Bootstrap | FR-008, FR-030 | 4-6 |
| EPIC-004 | Database Layer | FR-009, FR-010, FR-011 | 6-8 |
| EPIC-005 | Error Handling | FR-016, FR-017, FR-018, FR-019 | 5-7 |
| EPIC-006 | Rendering Stack | FR-020, FR-021, FR-022, FR-013 | 6-8 |
| EPIC-007 | Configuration & Defaults | FR-024, FR-025, FR-026, FR-027, FR-028, FR-029 | 6-8 |
| EPIC-008 | JSON & Static Assets | FR-014, FR-015, FR-023 | 3-4 |
| EPIC-009 | Documentation & Examples | FR-031, FR-032, FR-033, FR-034, FR-035 | 7-9 |
| EPIC-010 | CI & Distribution | FR-036, FR-037, FR-038 | 4-5 |
| EPIC-011 | v1.0 Security & Stability | FR-039 through FR-048 | 10-14 |
| | | **Total** | **64-86** |

---

## Appendix B: Prioritization Details

### Functional Requirements

| Priority | Count | Percentage |
|----------|-------|------------|
| Must Have (v0.1) | 38 | 79% |
| Should Have (v1.0) | 10 | 21% |
| Could Have | 0 | 0% |
| **Total** | **48** | **100%** |

### Non-Functional Requirements

| Priority | Count | Percentage |
|----------|-------|------------|
| Must Have | 11 | 92% |
| Should Have | 1 | 8% |
| **Total** | **12** | **100%** |

### Epic Priority Distribution

| Priority | Epics | Story Range |
|----------|-------|-------------|
| Must Have | 10 | 54-72 stories |
| Should Have | 1 | 10-14 stories |
| **Total** | **11** | **64-86 stories** |
