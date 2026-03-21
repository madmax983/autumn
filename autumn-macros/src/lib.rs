//! # Autumn Macros
//!
//! Proc macros for the Autumn web framework.
//!
//! This crate provides:
//! - Route annotation macros (`#[get]`, `#[post]`, etc.)
//! - The `routes![]` collection macro
//! - The `#[autumn::main]` entry point macro
//! - The `#[derive(Model)]` convenience macro
//!
//! Users should not depend on this crate directly — use `autumn` instead,
//! which re-exports everything.

use proc_macro::TokenStream;

/// Placeholder attribute macro for GET route handlers.
///
/// # Example (future)
///
/// ```ignore
/// #[get("/hello")]
/// async fn hello() -> &'static str {
///     "Hello, Autumn!"
/// }
/// ```
#[proc_macro_attribute]
pub fn get(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Placeholder: returns the item unchanged.
    // Real implementation comes in S-002.
    item
}
