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
///     fn render_error(&self, ctx: &ErrorContext) -> Markup {
///         html! {
///             html {
///                 body {
///                     h1 { (ctx.status.as_u16()) " — " (ctx.path) }
///                     a href="/" { "Go home" }
///                 }
///             }
///         }
///     }
/// }
/// ```
pub trait ErrorPageRenderer: Send + Sync + 'static {
    /// Renders a specific page for `404 Not Found` errors.
    ///
    /// By default, this delegates to the generic [`render_error`](Self::render_error)
    /// method. Implement this directly if you want a custom, branded "page not found"
    /// experience that differs from your generic error layout.
    #[must_use]
    fn render_404(&self, ctx: &ErrorContext) -> Markup {
        self.render_error(ctx)
    }

    /// Renders a specific page for `500 Internal Server Error`s.
    ///
    /// By default, this delegates to the generic [`render_error`](Self::render_error)
    /// method. Implement this directly if you want to emphasize things like an
    /// incident ID or a link to your status page when things go critically wrong.
    #[must_use]
    fn render_500(&self, ctx: &ErrorContext) -> Markup {
        self.render_error(ctx)
    }

    /// Renders a specific page for `422 Unprocessable Entity` validation errors.
    ///
    /// By default, this delegates to the generic [`render_error`](Self::render_error)
    /// method. Implement this directly to render the field-level `details` stored
    /// in the [`ErrorContext`] so users know exactly why their input was rejected.
    #[must_use]
    fn render_422(&self, ctx: &ErrorContext) -> Markup {
        self.render_error(ctx)
    }

    /// Renders a generic error page for any unhandled status code.
    ///
    /// This is the required fallback used by the default implementations of
    /// [`render_404`](Self::render_404), [`render_500`](Self::render_500), and
    /// [`render_422`](Self::render_422). Override this to provide a single
    /// unified template for all error codes.
    #[must_use]
    fn render_error(&self, ctx: &ErrorContext) -> Markup;
}
