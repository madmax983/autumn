//! # Autumn Macros
//!
//! Proc macros for the Autumn web framework.
//!
//! This crate provides:
//! - Route annotation macros (`#[get]`, `#[post]`, etc.)
//! - The `routes![]` collection macro
//! - The `#[autumn_web::main]` entry point macro (S-008)
//! - The `#[model]` attribute macro (S-018)
//!
//! Users should not depend on this crate directly — use `autumn-web` instead,
//! which re-exports everything.

mod collect;
mod main_macro;
mod model;
mod parse;
mod repository;
mod route;
mod routes_macro;
mod scheduled;
mod static_route;
mod static_routes_macro;
mod tasks_macro;

use proc_macro::TokenStream;

/// Annotate an async function as a GET route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns an [`autumn_web::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::get;
///
/// #[get("/hello")]
/// async fn hello() -> &'static str {
///     "Hello, Autumn!"
/// }
/// ```
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("GET", "get", attr.into(), item.into()).into()
}

/// Annotate an async function as a POST route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns an [`autumn_web::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::post;
///
/// #[post("/items")]
/// async fn create_item() -> &'static str {
///     "created"
/// }
/// ```
#[proc_macro_attribute]
pub fn post(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("POST", "post", attr.into(), item.into()).into()
}

/// Annotate an async function as a PUT route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns an [`autumn_web::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::put;
///
/// #[put("/items/{id}")]
/// async fn update_item() -> &'static str {
///     "updated"
/// }
/// ```
#[proc_macro_attribute]
pub fn put(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("PUT", "put", attr.into(), item.into()).into()
}

/// Annotate an async function as a DELETE route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns an [`autumn_web::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::delete;
///
/// #[delete("/items/{id}")]
/// async fn remove_item() -> &'static str {
///     "removed"
/// }
/// ```
#[proc_macro_attribute]
pub fn delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("DELETE", "delete", attr.into(), item.into()).into()
}

/// Collect annotated route handlers into a `Vec<Route>`.
///
/// Each handler must have been annotated with a route macro (`#[get]`,
/// `#[post]`, etc.) which generates a companion
/// `__autumn_route_info_{name}()` function.
///
/// # Example
///
/// ```ignore
/// use autumn_web::{get, post, routes};
///
/// #[get("/hello")]
/// async fn hello() -> &'static str { "hello" }
///
/// #[post("/create")]
/// async fn create() -> &'static str { "created" }
///
/// let all_routes = routes![hello, create];
/// ```
#[proc_macro]
pub fn routes(input: TokenStream) -> TokenStream {
    routes_macro::routes_macro(input.into()).into()
}

/// Set up the async runtime for an Autumn application.
///
/// This is a thin wrapper around `#[tokio::main]`. The real
/// framework setup happens in `autumn_web::app().run()`.
///
/// # Example
///
/// ```ignore
/// #[autumn_web::main]
/// async fn main() {
///     autumn_web::app()
///         .routes(routes![hello])
///         .run()
///         .await;
/// }
/// ```
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    main_macro::main_macro(item.into()).into()
}

/// Attribute macro for Autumn database models.
///
/// Applies Diesel (`Queryable`, `Selectable`, `Insertable`) and Serde
/// (`Serialize`, `Deserialize`) derives, plus a `#[diesel(table_name)]`
/// attribute. The table name can be specified explicitly or inferred
/// from the struct name by converting `PascalCase` to `snake_case`
/// and appending `s`.
///
/// # Examples
///
/// Explicit table name:
///
/// ```ignore
/// use autumn_web::model;
///
/// #[model(table = "users")]
/// pub struct User {
///     pub id: i64,
///     pub name: String,
/// }
/// ```
///
/// Inferred table name (`BlogPost` -> `blog_posts`):
///
/// ```ignore
/// use autumn_web::model;
///
/// #[model]
/// pub struct BlogPost {
///     pub id: i64,
///     pub title: String,
/// }
/// ```
#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    model::model_macro(attr.into(), item.into()).into()
}

/// Derive a repository with CRUD operations and derived queries.
///
/// Generates a `PgXxxRepository` struct implementing the annotated trait,
/// with auto-generated CRUD methods and query-by-name derived methods.
///
/// # Examples
///
/// ```ignore
/// use autumn_web::repository;
///
/// #[repository(Post)]
/// trait PostRepository {
///     fn find_by_published(published: bool) -> Vec<Post>;
/// }
/// ```
#[proc_macro_attribute]
pub fn repository(attr: TokenStream, item: TokenStream) -> TokenStream {
    repository::repository_macro(attr.into(), item.into()).into()
}

/// Declare a scheduled background task.
///
/// # Examples
///
/// ```ignore
/// #[scheduled(every = "5m", name = "cleanup")]
/// async fn cleanup(state: AppState) -> AutumnResult<()> { Ok(()) }
///
/// #[scheduled(cron = "0 0 0 * * *", name = "nightly")]
/// async fn nightly(state: AppState) -> AutumnResult<()> { Ok(()) }
/// ```
#[proc_macro_attribute]
pub fn scheduled(attr: TokenStream, item: TokenStream) -> TokenStream {
    scheduled::scheduled_macro(attr.into(), item.into()).into()
}

/// Annotate an async function as a statically pre-rendered GET route.
///
/// Like `#[get]`, this generates a route companion function. Additionally,
/// it generates a `__autumn_static_meta_{name}()` companion that registers
/// the route for static HTML generation at build time.
///
/// Phase 1: path parameters are **not** supported. Use `#[get]` for
/// parameterized routes.
///
/// # Example
///
/// ```ignore
/// use autumn_web::static_get;
///
/// #[static_get("/about")]
/// async fn about() -> &'static str {
///     "About us"
/// }
/// ```
#[proc_macro_attribute]
pub fn static_get(attr: TokenStream, item: TokenStream) -> TokenStream {
    static_route::static_get_macro(attr.into(), item.into()).into()
}

/// Collect `#[scheduled]` task handlers into a `Vec<TaskInfo>`.
///
/// ```ignore
/// let all_tasks = tasks![cleanup, nightly];
/// ```
#[proc_macro]
pub fn tasks(input: TokenStream) -> TokenStream {
    tasks_macro::tasks_macro(input.into()).into()
}

/// Collect `#[static_get]` handlers into a `Vec<StaticRouteMeta>`.
///
/// ```ignore
/// use autumn_web::prelude::*;
///
/// #[static_get("/about")]
/// async fn about() -> &'static str { "About" }
///
/// let metas = static_routes![about];
/// ```
#[proc_macro]
pub fn static_routes(input: TokenStream) -> TokenStream {
    static_routes_macro::static_routes_macro(input.into()).into()
}
