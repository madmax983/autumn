//! Convenience re-exports for Autumn applications.
//!
//! Import everything commonly needed with a single glob:
//!
//! ```rust,ignore
//! use autumn::prelude::*;
//! ```
//!
//! This brings route macros, rendering types, extractors, and error
//! types into scope. For less common types (config, middleware,
//! upstream crate access), use targeted imports from `autumn::config`,
//! `autumn::middleware`, or `autumn::reexports`.

// ── Route macros ─────────────────────────────────────────────────
/// HTTP method route macros, main macro, and route collection.
pub use autumn_macros::{delete, get, main, post, put, routes};

// ── Rendering ────────────────────────────────────────────────────
/// Maud HTML templating types.
pub use maud::{Markup, PreEscaped, html};

// ── Extractors ───────────────────────────────────────────────────
/// Database connection extractor.
pub use crate::db::Db;
/// JSON request/response type.
pub use axum::Json;
/// Form data extractor.
pub use axum::extract::Form;

// ── Error handling ───────────────────────────────────────────────
/// Framework error and result types.
pub use crate::error::{AutumnError, AutumnResult};

// ── Application state ────────────────────────────────────────────
/// Shared application state (for custom extractors).
pub use crate::AppState;

#[cfg(test)]
mod tests {
    use super::*;

    // Verify types are in scope by using them in type position
    #[allow(dead_code)]
    async fn handler_using_prelude(db: Db) -> AutumnResult<Markup> {
        Ok(html! { "test" })
    }

    #[allow(dead_code)]
    fn json_handler() -> Json<&'static str> {
        Json("ok")
    }

    #[test]
    fn prelude_types_are_accessible() {
        // This test exists to verify compilation — if it compiles, the prelude works
        let _: fn(
            Db,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = AutumnResult<Markup>> + Send>,
        > = |_| todo!();
    }
}
