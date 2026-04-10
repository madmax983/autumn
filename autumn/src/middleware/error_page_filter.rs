//! Exception filter that renders HTML error pages for browser requests.
//!
//! When a request's `Accept` header prefers HTML over JSON (typical for
//! browser navigation), this filter replaces the default JSON error
//! response with a styled HTML error page. JSON API requests are left
//! untouched.
//!
//! In dev mode, a Next.js-style error badge is injected into the HTML
//! response for quick debugging.

use axum::response::{IntoResponse, Response};

use crate::error_pages::dev_badge::{self, DevBadgeContext};
use crate::error_pages::renderer::ErrorContext;
use crate::error_pages::{self, SharedRenderer};
use crate::middleware::exception_filter::{AutumnErrorInfo, ExceptionFilter};

/// Exception filter that renders HTML error pages for browser requests.
///
/// Injected automatically by the framework. When a request has an `Accept`
/// header indicating HTML preference (browser navigation), the JSON error
/// response is replaced with a styled HTML page.
///
/// In dev profile, the HTML page includes a floating error badge overlay.
pub struct ErrorPageFilter {
    pub renderer: SharedRenderer,
    pub is_dev: bool,
}

impl ExceptionFilter for ErrorPageFilter {
    fn filter(&self, error: &AutumnErrorInfo, response: Response) -> Response {
        // Check if the original request wanted HTML (stored in extensions
        // by the error page middleware layer).
        let wants_html = response
            .extensions()
            .get::<WantsHtml>()
            .is_some_and(|w| w.0);

        if !wants_html {
            return response;
        }

        let request_id = response
            .extensions()
            .get::<ErrorPageRequestContext>()
            .and_then(|ctx| ctx.request_id.clone());

        let path = response
            .extensions()
            .get::<ErrorPageRequestContext>()
            .map(|ctx| ctx.path.clone())
            .unwrap_or_default();

        let ctx = ErrorContext {
            status: error.status,
            message: error.message.clone(),
            path,
            request_id: request_id.clone(),
            details: error.details.clone(),
            is_dev: self.is_dev,
        };

        let mut html_body =
            error_pages::render_error_page(self.renderer.as_ref(), error.status, &ctx)
                .into_string();

        // In dev mode, inject the error badge before </body>
        if self.is_dev {
            let badge_ctx = DevBadgeContext {
                status_code: error.status.as_u16(),
                status_reason: error
                    .status
                    .canonical_reason()
                    .unwrap_or("Error")
                    .to_string(),
                message: error.message.clone(),
                path: ctx.path,
                request_id,
                source_location: None,
            };
            let badge = dev_badge::dev_error_badge_html(&badge_ctx).into_string();
            if let Some(pos) = html_body.rfind("</body>") {
                html_body.insert_str(pos, &badge);
            } else {
                html_body.push_str(&badge);
            }
        }

        let mut resp = (
            error.status,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html_body,
        )
            .into_response();

        // Re-attach error info so downstream filters still see it
        resp.extensions_mut().insert(error.clone());

        resp
    }
}

/// Marker stored in response extensions to indicate the original request
/// preferred HTML responses.
#[derive(Clone, Debug)]
pub struct WantsHtml(pub bool);

/// Request context stored in response extensions for the error page filter.
#[derive(Clone, Debug)]
pub struct ErrorPageRequestContext {
    pub path: String,
    pub request_id: Option<String>,
}

/// Tower layer that annotates requests with Accept header preference
/// so the error page filter knows whether to render HTML or pass through JSON.
///
/// This layer runs before the exception filter and stores [`WantsHtml`]
/// and [`ErrorPageRequestContext`] in the response extensions.
#[derive(Clone)]
pub struct ErrorPageContextLayer;

impl<S> tower::Layer<S> for ErrorPageContextLayer {
    type Service = ErrorPageContextService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ErrorPageContextService { inner }
    }
}

/// Tower [`Service`](tower::Service) produced by [`ErrorPageContextLayer`].
///
/// Wraps an inner service and extracts information about whether the client
/// prefers an HTML response (via the `Accept` header), passing it along in the
/// request extensions for downstream error handling.
#[derive(Clone)]
pub struct ErrorPageContextService<S> {
    inner: S,
}

impl<S, ReqBody> tower::Service<axum::http::Request<ReqBody>> for ErrorPageContextService<S>
where
    S: tower::Service<axum::http::Request<ReqBody>, Response = Response>,
{
    type Response = Response;
    type Error = S::Error;
    type Future = ErrorPageContextFuture<S::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: axum::http::Request<ReqBody>) -> Self::Future {
        let wants_html = accepts_html(&req);
        let path = req.uri().path().to_string();
        let request_id = req
            .extensions()
            .get::<crate::middleware::RequestId>()
            .map(std::string::ToString::to_string);

        ErrorPageContextFuture {
            inner: self.inner.call(req),
            wants_html,
            path,
            request_id,
        }
    }
}

pin_project_lite::pin_project! {
    pub struct ErrorPageContextFuture<F> {
        #[pin]
        inner: F,
        wants_html: bool,
        path: String,
        request_id: Option<String>,
    }
}

impl<F, E> std::future::Future for ErrorPageContextFuture<F>
where
    F: std::future::Future<Output = Result<Response, E>>,
{
    type Output = Result<Response, E>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();
        match this.inner.poll(cx) {
            std::task::Poll::Ready(Ok(mut response)) => {
                response
                    .extensions_mut()
                    .insert(WantsHtml(*this.wants_html));
                response.extensions_mut().insert(ErrorPageRequestContext {
                    path: this.path.clone(),
                    request_id: this.request_id.clone(),
                });
                std::task::Poll::Ready(Ok(response))
            }
            other => other,
        }
    }
}

/// Check if the request's Accept header indicates a preference for HTML.
///
/// Returns `true` for typical browser requests (`text/html` or `*/*`),
/// `false` for API requests (`application/json`).
fn accepts_html<B>(req: &axum::http::Request<B>) -> bool {
    let accept = req
        .headers()
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // If no Accept header, default to JSON (API-first).
    if accept.is_empty() {
        return false;
    }

    // Simple heuristic: if text/html appears before application/json,
    // or if text/html is present and application/json is not, prefer HTML.
    let html_pos = accept.find("text/html");
    let json_pos = accept.find("application/json");

    match (html_pos, json_pos) {
        (Some(_), None) => true,
        (Some(h), Some(j)) => h < j,
        (None, Some(_)) => false,
        // `*/*` without specific types -- default to HTML for browsers
        (None, None) => accept.contains("*/*"),
    }
}

/// 404 fallback handler for unmatched routes.
///
/// This is mounted as the router's fallback so unmatched routes get proper
/// error pages instead of Axum's default plain-text "Not Found".
pub async fn fallback_404_handler(uri: axum::http::Uri) -> crate::error::AutumnError {
    crate::error::AutumnError::not_found_msg(format!("No route matches {}", uri.path()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;

    #[test]
    fn accepts_html_for_browser() {
        let req = Request::builder()
            .header("accept", "text/html,application/xhtml+xml,*/*;q=0.8")
            .body(Body::empty())
            .unwrap();
        assert!(accepts_html(&req));
    }

    #[test]
    fn rejects_html_for_json_api() {
        let req = Request::builder()
            .header("accept", "application/json")
            .body(Body::empty())
            .unwrap();
        assert!(!accepts_html(&req));
    }

    #[test]
    fn rejects_html_for_empty_accept() {
        let req = Request::builder().body(Body::empty()).unwrap();
        assert!(!accepts_html(&req));
    }

    #[test]
    fn accepts_html_for_wildcard() {
        let req = Request::builder()
            .header("accept", "*/*")
            .body(Body::empty())
            .unwrap();
        assert!(accepts_html(&req));
    }

    #[test]
    fn prefers_json_when_json_first() {
        let req = Request::builder()
            .header("accept", "application/json, text/html")
            .body(Body::empty())
            .unwrap();
        assert!(!accepts_html(&req));
    }

    #[test]
    fn prefers_html_when_html_first() {
        let req = Request::builder()
            .header("accept", "text/html, application/json")
            .body(Body::empty())
            .unwrap();
        assert!(accepts_html(&req));
    }

    // ── Integration tests with the full middleware pipeline ──────

    use std::sync::Arc;

    use axum::Router;
    use axum::routing::get;
    use http::StatusCode;
    use tower::ServiceExt;

    use crate::error::AutumnError;
    use crate::error_pages;
    use crate::middleware::exception_filter::ExceptionFilterLayer;

    /// Helper: build a router with the error page filter and context layer.
    fn test_router_with_error_pages(is_dev: bool) -> Router {
        let renderer = error_pages::default_renderer();
        let error_page_filter = ErrorPageFilter { renderer, is_dev };
        let filters: Vec<Arc<dyn crate::middleware::ExceptionFilter>> =
            vec![Arc::new(error_page_filter)];

        Router::new()
            .route("/ok", get(|| async { "all good" }))
            .route(
                "/fail",
                get(|| async { Err::<String, AutumnError>(AutumnError::not_found_msg("gone")) }),
            )
            .route(
                "/boom",
                get(|| async {
                    Err::<String, AutumnError>(AutumnError::from(std::io::Error::other(
                        "internal failure",
                    )))
                }),
            )
            .fallback(fallback_404_handler)
            .layer(ErrorPageContextLayer)
            .layer(ExceptionFilterLayer::new(filters))
    }

    #[tokio::test]
    async fn html_404_returns_styled_page() {
        let app = test_router_with_error_pages(false);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("<!DOCTYPE html>"), "should be HTML");
        assert!(body_str.contains("404"), "should contain status code");
        assert!(
            body_str.contains("Page not found"),
            "should contain not found message"
        );
    }

    #[tokio::test]
    async fn html_500_returns_styled_page() {
        let app = test_router_with_error_pages(false);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/boom")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("<!DOCTYPE html>"), "should be HTML");
        assert!(body_str.contains("500"), "should contain status code");
        assert!(
            !body_str.contains("internal failure"),
            "must NOT show error details in prod"
        );
    }

    #[tokio::test]
    async fn json_api_gets_json_errors() {
        let app = test_router_with_error_pages(false);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/fail")
                    .header("accept", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.contains("application/json"),
            "JSON API should get JSON response, got: {ct}"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["status"], 404);
    }

    #[tokio::test]
    async fn dev_badge_appears_in_dev_mode() {
        let app = test_router_with_error_pages(true);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            body_str.contains("autumn-dev-error-badge"),
            "dev mode should include error badge"
        );
    }

    #[tokio::test]
    async fn dev_badge_hidden_in_prod_mode() {
        let app = test_router_with_error_pages(false);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !body_str.contains("autumn-dev-error-badge"),
            "prod mode must NOT include error badge"
        );
    }

    #[tokio::test]
    async fn custom_renderer_works() {
        use crate::error_pages::{ErrorContext, ErrorPageRenderer};
        use maud::{Markup, html};

        struct CustomPages;
        impl ErrorPageRenderer for CustomPages {
            fn render_error(&self, ctx: &ErrorContext) -> Markup {
                html! {
                    h1 { "CUSTOM " (ctx.status.as_u16()) }
                }
            }
        }

        let renderer: crate::error_pages::SharedRenderer = Arc::new(CustomPages);
        let error_page_filter = ErrorPageFilter {
            renderer,
            is_dev: false,
        };
        let filters: Vec<Arc<dyn crate::middleware::ExceptionFilter>> =
            vec![Arc::new(error_page_filter)];

        let app = Router::new()
            .route(
                "/err",
                get(|| async { Err::<String, AutumnError>(AutumnError::not_found_msg("nope")) }),
            )
            .layer(ErrorPageContextLayer)
            .layer(ExceptionFilterLayer::new(filters));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/err")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            body_str.contains("CUSTOM 404"),
            "should use custom renderer, got: {body_str}"
        );
    }

    #[tokio::test]
    async fn success_responses_not_affected() {
        let app = test_router_with_error_pages(false);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ok")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"all good");
    }
}
