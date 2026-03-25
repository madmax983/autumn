//! Convenience re-exports for Autumn applications.
//!
//! Import everything commonly needed with a single glob:
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! ```
//!
//! This brings the following into scope:
//!
//! | Category | Items |
//! |----------|-------|
//! | Route macros | [`get`], [`post`], [`put`], [`delete`], [`routes`], [`main`] |
//! | HTML rendering | [`Markup`], [`PreEscaped`], [`html!`](maud::html) |
//! | Extractors | [`Db`], [`Json`], [`Form`] |
//! | Error handling | [`AutumnError`], [`AutumnResult`] |
//! | State | [`AppState`] |
//!
//! For less common types (configuration, middleware, upstream crate access),
//! use targeted imports from [`autumn_web::config`](crate::config),
//! [`autumn_web::middleware`](crate::middleware), or
//! [`autumn_web::reexports`](crate::reexports).

// ── Route macros ─────────────────────────────────────────────────
/// HTTP method route macros, main macro, and route collection.
pub use autumn_macros::{delete, get, main, post, put, routes};

// ── Rendering ────────────────────────────────────────────────────
/// Maud HTML templating types.
#[cfg(feature = "maud")]
pub use maud::{Markup, PreEscaped, html};

// ── Extractors ───────────────────────────────────────────────────
/// Database connection extractor.
#[cfg(feature = "db")]
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

    // Verify key types are in scope by using them in type position.
    // These are compile-time checks — if this module compiles, the prelude works.
    #[cfg(all(feature = "db", feature = "maud"))]
    #[allow(dead_code, clippy::unnecessary_wraps)]
    fn _handler_using_prelude(_db: Db) -> AutumnResult<Markup> {
        Ok(html! { "test" })
    }

    #[allow(dead_code)]
    fn _json_handler() -> Json<&'static str> {
        Json("ok")
    }

    #[test]
    fn prelude_types_are_accessible() {
        // Compilation is the test — verify a few types exist at runtime too
        #[cfg(feature = "db")]
        let _state = AppState { pool: None };
        #[cfg(not(feature = "db"))]
        let _state = AppState {};
        let _err: AutumnResult<()> = Ok(());
    }
}
