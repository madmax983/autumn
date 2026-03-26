//! # Autumn
//!
//! An opinionated, convention-over-configuration web framework for Rust.
//!
//! Autumn assembles proven Rust crates ([Axum], [Maud], [Diesel], htmx, Tailwind)
//! into a Spring Boot-style developer experience with proc-macro-driven
//! conventions and escape hatches at every level.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//!
//! #[get("/")]
//! async fn index() -> Markup {
//!     html! { h1 { "Hello, Autumn!" } }
//! }
//!
//! #[autumn_web::main]
//! async fn main() {
//!     autumn_web::app()
//!         .routes(routes![index])
//!         .run()
//!         .await;
//! }
//! ```
//!
//! ## Architecture overview
//!
//! | Layer | Crate | Purpose |
//! |-------|-------|---------|
//! | HTTP server | [Axum] | Routing, extractors, middleware |
//! | HTML templates | [Maud] | Type-safe, compiled HTML via `html!` macro |
//! | Database | [Diesel] | Async Postgres via `diesel-async` + deadpool |
//! | Client interactivity | htmx | Embedded JS served at `/static/js/htmx.min.js` |
//! | Styling | Tailwind CSS | Downloaded + managed by `autumn-cli` |
//!
//! ## Modules
//!
//! - [`mod@app`] -- Application builder for configuring and launching the server.
//! - [`config`] -- Layered configuration: defaults, `autumn.toml`, env overrides.
//! - [`db`] -- Database connection pool and the [`Db`] request extractor.
//! - [`error`] -- Framework error type ([`AutumnError`]) and result alias.
//! - [`extract`] -- Re-exported Axum extractors ([`Form`](axum::extract::Form),
//!   [`Json`], [`Path`](axum::extract::Path), [`Query`](axum::extract::Query)).
//! - [`health`] -- Auto-mounted health check endpoint.
//! - [`logging`] -- Structured logging via `tracing-subscriber`.
//! - [`middleware`] -- Built-in middleware (request IDs).
//! - [`prelude`] -- Glob import for the most common types.
//! - [`route`] -- Route descriptor used by macro-generated code.
//!
//! ## Zero-config defaults
//!
//! An Autumn app runs out of the box with no configuration file. Every
//! setting has a sensible default (port 3000, `info` log level, etc.).
//! Override via `autumn.toml` or `AUTUMN_*` environment variables.
//! See [`config::AutumnConfig`] for the full list.
//!
//! [Axum]: https://docs.rs/axum
//! [Maud]: https://maud.lambda.xyz
//! [Diesel]: https://diesel.rs

pub mod app;
pub mod config;
#[cfg(feature = "db")]
pub mod db;
pub mod error;
pub mod extract;
pub mod health;
#[cfg(feature = "htmx")]
pub(crate) mod htmx;
pub mod logging;
pub mod middleware;
pub mod prelude;
pub mod route;

/// Create a new [`app::AppBuilder`] for configuring and launching an Autumn server.
///
/// This is the primary entry point for every Autumn application.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/")]
/// async fn index() -> &'static str { "hello" }
///
/// #[autumn_web::main]
/// async fn main() {
///     autumn_web::app()
///         .routes(routes![index])
///         .run()
///         .await;
/// }
/// ```
pub use app::app;
/// Async database connection extractor.
///
/// Declare `db: Db` in a handler signature to get a pooled Postgres
/// connection. See [`db::Db`] for full documentation and examples.
#[cfg(feature = "db")]
pub use db::Db;

/// Framework error type and result alias.
///
/// [`AutumnError`] wraps any `Error + Send + Sync` with an HTTP status code.
/// [`AutumnResult<T>`] is `Result<T, AutumnError>`.
/// See the [`error`] module for details.
pub use error::{AutumnError, AutumnResult};
/// htmx version string embedded in the binary.
///
/// Useful for cache-busting or diagnostic logging. The corresponding
/// minified JS is served automatically at `/static/js/htmx.min.js`.
#[cfg(feature = "htmx")]
pub use htmx::HTMX_VERSION;

// ── Proc-macro re-exports ──────────────────────────────────────────

/// Annotate an async function as a `DELETE` route handler.
///
/// Generates a companion function that returns a [`route::Route`]
/// pairing the path with an Axum handler. In debug builds
/// `#[axum::debug_handler]` is applied automatically for better error
/// messages (zero cost in release).
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[delete("/items/{id}")]
/// async fn remove_item() -> &'static str {
///     "removed"
/// }
/// ```
pub use autumn_macros::delete;

/// Annotate an async function as a `GET` route handler.
///
/// Generates a companion function that returns a [`route::Route`]
/// pairing the path with an Axum handler. In debug builds
/// `#[axum::debug_handler]` is applied automatically for better error
/// messages (zero cost in release).
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/hello")]
/// async fn hello() -> &'static str {
///     "Hello, Autumn!"
/// }
/// ```
pub use autumn_macros::get;

/// Set up the Tokio async runtime for an Autumn application.
///
/// A thin wrapper around `#[tokio::main]`. The real framework setup
/// happens inside [`app::AppBuilder::run`].
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/")]
/// async fn index() -> &'static str { "hi" }
///
/// #[autumn_web::main]
/// async fn main() {
///     autumn_web::app()
///         .routes(routes![index])
///         .run()
///         .await;
/// }
/// ```
pub use autumn_macros::main;

/// Derive Diesel and Serde traits for a database model struct.
///
/// Applies `Queryable`, `Selectable`, `Insertable`, `Serialize`, and
/// `Deserialize` derives plus a `#[diesel(table_name = ...)]` attribute.
/// The table name is either specified explicitly or inferred from the
/// struct name (`PascalCase` -> `snake_case` + `s`).
///
/// # Examples
///
/// Explicit table name:
///
/// ```rust,ignore
/// use autumn_web::model;
///
/// #[model(table = "users")]
/// pub struct User {
///     pub id: i32,
///     pub name: String,
/// }
/// ```
///
/// Inferred table name (`BlogPost` -> `blog_posts`):
///
/// ```rust,ignore
/// use autumn_web::model;
///
/// #[model]
/// pub struct BlogPost {
///     pub id: i32,
///     pub title: String,
/// }
/// ```
#[cfg(feature = "db")]
pub use autumn_macros::model;

/// Annotate an async function as a `POST` route handler.
///
/// Generates a companion function that returns a [`route::Route`]
/// pairing the path with an Axum handler. In debug builds
/// `#[axum::debug_handler]` is applied automatically for better error
/// messages (zero cost in release).
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[post("/items")]
/// async fn create_item() -> &'static str {
///     "created"
/// }
/// ```
pub use autumn_macros::post;

/// Annotate an async function as a `PUT` route handler.
///
/// Generates a companion function that returns a [`route::Route`]
/// pairing the path with an Axum handler. In debug builds
/// `#[axum::debug_handler]` is applied automatically for better error
/// messages (zero cost in release).
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[put("/items/{id}")]
/// async fn update_item() -> &'static str {
///     "updated"
/// }
/// ```
pub use autumn_macros::put;

/// Collect route-annotated handlers into a `Vec<Route>`.
///
/// Each handler must have been annotated with a route macro ([`get`],
/// [`post`], [`put`], [`delete`]) which generates a companion
/// `__autumn_route_info_{name}()` function.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/hello")]
/// async fn hello() -> &'static str { "hello" }
///
/// #[post("/create")]
/// async fn create() -> &'static str { "created" }
///
/// # #[autumn_web::main]
/// # async fn main() {
/// let all_routes = routes![hello, create];
/// autumn_web::app().routes(all_routes).run().await;
/// # }
/// ```
pub use autumn_macros::routes;

// ── Maud re-exports ────────────────────────────────────────────────

/// Rendered HTML fragment produced by the [`html!`] macro.
///
/// This is the standard return type for handlers that render HTML.
/// Re-exported from [Maud](https://maud.lambda.xyz).
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/")]
/// async fn index() -> Markup {
///     html! { h1 { "Welcome" } }
/// }
/// ```
#[cfg(feature = "maud")]
pub use maud::Markup;

/// Wrap a pre-escaped string so Maud renders it verbatim.
///
/// Use this when you have HTML that was already escaped or generated
/// by another system and you want to embed it in a Maud template
/// without double-escaping.
///
/// Re-exported from [Maud](https://maud.lambda.xyz).
///
/// # Examples
///
/// ```rust
/// use autumn_web::PreEscaped;
///
/// let raw_html = PreEscaped("<em>already escaped</em>".to_string());
/// ```
#[cfg(feature = "maud")]
pub use maud::PreEscaped;

/// Type-safe HTML templating macro.
///
/// Produces a [`Markup`] value containing compiled HTML.
/// Re-exported from [Maud](https://maud.lambda.xyz). See the
/// [Maud book](https://maud.lambda.xyz) for full syntax reference.
///
/// # Examples
///
/// ```rust
/// use autumn_web::html;
///
/// let greeting = "world";
/// let page = html! {
///     h1 { "Hello, " (greeting) "!" }
/// };
/// ```
#[cfg(feature = "maud")]
pub use maud::html;

/// JSON request body extractor and response type.
///
/// When used as a handler parameter, deserializes the request body as JSON.
/// When returned from a handler, serializes the value as JSON with
/// `Content-Type: application/json`.
///
/// Re-exported from [Axum](https://docs.rs/axum). See
/// [`axum::Json`] for full documentation.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Deserialize)]
/// struct CreateItem { name: String }
///
/// #[derive(Serialize)]
/// struct Item { id: i32, name: String }
///
/// #[post("/items")]
/// async fn create(Json(input): Json<CreateItem>) -> Json<Item> {
///     Json(Item { id: 1, name: input.name })
/// }
/// ```
pub use crate::extract::Json;

/// Re-exports of upstream crates used in macro-generated code.
///
/// These are public so that code generated by `autumn-macros` can reference
/// them as `autumn_web::reexports::axum`, etc. without requiring the user to
/// add those crates as direct dependencies.
///
/// **For advanced use cases only.** Prefer the types re-exported in
/// [`prelude`] or at the crate root. Reach into `reexports` when you
/// need direct access to the underlying framework types (e.g.,
/// `autumn_web::reexports::axum::Router` for custom middleware).
///
/// # Available crates
///
/// | Crate | Re-exported as | Use case |
/// |-------|---------------|----------|
/// | `axum` | `autumn_web::reexports::axum` | Custom routers, middleware, extractors |
/// | `diesel` | `autumn_web::reexports::diesel` | Raw Diesel queries, schema types |
/// | `http` | `autumn_web::reexports::http` | HTTP types (`StatusCode`, `Method`, headers) |
/// | `tokio` | `autumn_web::reexports::tokio` | Async runtime, spawn, timers |
pub mod reexports {
    pub use axum;
    #[cfg(feature = "db")]
    pub use diesel;
    pub use http;
    pub use tokio;
}

/// Shared application state passed to all route handlers.
///
/// Holds framework-managed resources such as the database connection pool.
/// Axum requires handler state to be [`Clone`], so internal resources use
/// `Arc` or are already cheaply cloneable (`deadpool::Pool` is `Arc`-wrapped
/// internally).
///
/// This struct is normally constructed by [`app::AppBuilder::run`] and
/// should not need to be created manually. It is public so that custom
/// Axum extractors can access framework resources via
/// `State<AppState>`.
///
/// # Examples
///
/// ```rust
/// use autumn_web::AppState;
///
/// // State without a database (e.g., for testing)
/// let state = AppState { pool: None };
/// ```
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool, or `None` when no `database.url` is configured.
    #[cfg(feature = "db")]
    pub pool:
        Option<diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(feature = "db")]
        {
            f.debug_struct("AppState")
                .field(
                    "pool",
                    &self
                        .pool
                        .as_ref()
                        .map(|p| format!("Pool(max={})", p.status().max_size)),
                )
                .finish()
        }
        #[cfg(not(feature = "db"))]
        {
            f.debug_struct("AppState").finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_state_debug_without_pool() {
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
        };
        let debug = format!("{state:?}");
        assert!(debug.contains("AppState"));
    }

    #[cfg(feature = "db")]
    #[test]
    fn app_state_debug_with_pool() {
        let config = config::DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            pool_size: 5,
            ..Default::default()
        };
        let pool = db::create_pool(&config).unwrap().unwrap();
        let state = AppState { pool: Some(pool) };
        let debug = format!("{state:?}");
        assert!(debug.contains("Pool(max=5)"));
    }

    fn require_clone<T: Clone>(t: &T) -> T {
        t.clone()
    }

    #[test]
    fn app_state_is_clone() {
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
        };
        let _cloned = require_clone(&state);
    }

    #[test]
    fn app_fn_creates_builder() {
        let builder = app::app();
        // Just verify it compiles and can accept routes
        let _builder = builder.routes(vec![]);
    }
}
