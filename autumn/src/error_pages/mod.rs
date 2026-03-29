//! Styled error pages and dev-mode error badge overlay.
//!
//! Autumn provides default HTML error pages for common HTTP error codes
//! (404, 422, 500) and a Next.js-style error badge overlay in dev mode.
//!
//! # Error page override
//!
//! Implement [`ErrorPageRenderer`] to provide custom error pages:
//!
//! ```rust,no_run
//! use autumn_web::error_pages::{ErrorPageRenderer, ErrorContext};
//! use maud::{Markup, html};
//!
//! struct MyErrorPages;
//!
//! impl ErrorPageRenderer for MyErrorPages {
//!     fn render_404(&self, ctx: &ErrorContext) -> Markup {
//!         html! {
//!             h1 { "Custom 404 - " (ctx.path) " not found" }
//!         }
//!     }
//! }
//! ```
//!
//! Register it on the app builder:
//!
//! ```rust,ignore
//! autumn_web::app()
//!     .error_pages(MyErrorPages)
//!     .routes(routes![...])
//!     .run()
//!     .await;
//! ```
//!
//! # Dev error badge
//!
//! In dev profile, error responses automatically include a floating badge
//! in the bottom-left corner showing status code, error type, and message.
//! The badge uses inline CSS so it works even when Tailwind is unavailable.
//! It is **never** shown in production.

mod defaults;
pub(crate) mod dev_badge;
pub(crate) mod renderer;

pub use defaults::DefaultErrorPages;
pub use renderer::{ErrorContext, ErrorPageRenderer};

use axum::http::StatusCode;
use maud::Markup;
use std::sync::Arc;

/// Render an error page using the given renderer (or defaults).
///
/// Returns the full HTML response body for the given status code.
pub(crate) fn render_error_page(
    renderer: &dyn ErrorPageRenderer,
    status: StatusCode,
    ctx: &ErrorContext,
) -> Markup {
    match status {
        StatusCode::NOT_FOUND => renderer.render_404(ctx),
        StatusCode::UNPROCESSABLE_ENTITY => renderer.render_422(ctx),
        StatusCode::INTERNAL_SERVER_ERROR => renderer.render_500(ctx),
        _ => renderer.render_error(ctx),
    }
}

/// Shared error page renderer stored in app state / middleware.
pub(crate) type SharedRenderer = Arc<dyn ErrorPageRenderer>;

/// Create the default shared renderer.
pub(crate) fn default_renderer() -> SharedRenderer {
    Arc::new(DefaultErrorPages)
}
