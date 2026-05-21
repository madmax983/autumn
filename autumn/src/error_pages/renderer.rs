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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use maud::html;

    struct DummyRenderer;

    impl ErrorPageRenderer for DummyRenderer {
        fn render_error(&self, ctx: &ErrorContext) -> Markup {
            html! {
                div {
                    "Dummy: " (ctx.status.as_u16())
                }
            }
        }
    }

    #[test]
    fn default_renderer_fallbacks_to_render_error() {
        let renderer = DummyRenderer;
        let ctx = ErrorContext {
            status: StatusCode::NOT_FOUND,
            message: "Not found".to_string(),
            path: "/missing".to_string(),
            request_id: None,
            details: None,
            is_dev: false,
        };

        // These default method implementations should simply delegate to render_error
        let not_found = renderer.render_404(&ctx).into_string();
        assert_eq!(not_found, "<div>Dummy: 404</div>");

        let mut ctx_500 = ctx.clone();
        ctx_500.status = StatusCode::INTERNAL_SERVER_ERROR;
        let internal_server = renderer.render_500(&ctx_500).into_string();
        assert_eq!(internal_server, "<div>Dummy: 500</div>");

        let mut ctx_422 = ctx;
        ctx_422.status = StatusCode::UNPROCESSABLE_ENTITY;
        let unprocessable = renderer.render_422(&ctx_422).into_string();
        assert_eq!(unprocessable, "<div>Dummy: 422</div>");
    }
}
