//! # Autumn Macros
//!
//! Proc macros for the Autumn web framework.
//!
//! This crate provides:
//! - Route annotation macros (`#[get]`, `#[post]`, etc.)
//! - The `routes![]` collection macro (S-005)
//! - The `#[autumn::main]` entry point macro (S-008)
//! - The `#[derive(Model)]` convenience macro (S-018)
//!
//! Users should not depend on this crate directly — use `autumn` instead,
//! which re-exports everything.

mod route;

use proc_macro::TokenStream;

/// Annotate an async function as a GET route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns an [`autumn::route::Route`] pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is auto-applied
/// for better error messages.
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
