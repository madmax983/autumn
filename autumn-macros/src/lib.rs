//! # Autumn Macros
//!
//! Proc macros for the Autumn web framework.
//!
//! This crate provides:
//! - Route annotation macros (`#[get]`, `#[post]`, etc.)
//! - The `routes![]` collection macro
//! - The `#[autumn::main]` entry point macro (S-008)
//! - The `#[derive(Model)]` convenience macro (S-018)
//!
//! Users should not depend on this crate directly — use `autumn` instead,
//! which re-exports everything.

mod main_macro;
mod route;
mod routes_macro;

use proc_macro::TokenStream;

/// Annotate an async function as a GET route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns an [`autumn::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn::get;
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
/// returns an [`autumn::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn::post;
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
/// returns an [`autumn::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn::put;
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
/// returns an [`autumn::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn::delete;
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
/// use autumn::{get, post, routes};
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
/// framework setup happens in `autumn::app().run()`.
///
/// # Example
///
/// ```ignore
/// #[autumn::main]
/// async fn main() {
///     autumn::app()
///         .routes(routes![hello])
///         .run()
///         .await;
/// }
/// ```
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    main_macro::main_macro(item.into()).into()
}
