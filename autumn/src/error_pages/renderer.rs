//! Error page renderer trait and context.

use axum::http::StatusCode;
use maud::Markup;
use std::collections::HashMap;

/// Context passed to error page renderers.
///
/// Contains all the information available about the error, allowing
/// renderers to produce rich error pages.
#[derive(Debug, Clone)]
pub struct ErrorContext {
    /// The HTTP status code (e.g., 404, 500).
    pub status: StatusCode,
    /// Human-readable error message.
    pub message: String,
    /// The request path that triggered the error.
    pub path: String,
    /// Request ID from the `X-Request-Id` header (if available).
    pub request_id: Option<String>,
    /// Field-level validation details (for 422 errors).
    pub details: Option<HashMap<String, Vec<String>>>,
    /// Whether the app is running in dev mode.
    pub is_dev: bool,
}

/// Trait for providing custom error pages.
///
/// Implement this trait to override the default error pages. Each method
/// receives an [`ErrorContext`] with information about the error.
///
/// The default implementation (via [`super::DefaultErrorPages`]) renders
/// styled HTML pages using Maud and Tailwind.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::error_pages::{ErrorPageRenderer, ErrorContext};
/// use maud::{Markup, html};
///
/// struct BrandedErrors;
///
/// impl ErrorPageRenderer for BrandedErrors {
///     fn render_404(&self, ctx: &ErrorContext) -> Markup {
///         html! {
///             html {
///                 body {
///                     h1 { "Oops! " (ctx.path) " was not found." }
///                     a href="/" { "Go home" }
///                 }
///             }
///         }
///     }
/// }
/// ```
pub trait ErrorPageRenderer: Send + Sync + 'static {
    /// Render a 404 Not Found page.
    fn render_404(&self, ctx: &ErrorContext) -> Markup {
        self.render_error(ctx)
    }

    /// Render a 500 Internal Server Error page.
    fn render_500(&self, ctx: &ErrorContext) -> Markup {
        self.render_error(ctx)
    }

    /// Render a 422 Unprocessable Entity page (validation errors).
    fn render_422(&self, ctx: &ErrorContext) -> Markup {
        self.render_error(ctx)
    }

    /// Render a generic error page for any status code.
    ///
    /// This is the fallback used by the default implementations of
    /// `render_404`, `render_500`, and `render_422`. Override this
    /// to provide a single template for all error codes.
    fn render_error(&self, ctx: &ErrorContext) -> Markup;
}
