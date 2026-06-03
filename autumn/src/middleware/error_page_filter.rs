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
    pub parameter_filter: crate::log::filter::ParameterFilter,
}

impl ExceptionFilter for ErrorPageFilter {
    fn filter(&self, error: &AutumnErrorInfo, response: Response) -> Response {
        let wants_html = response
            .extensions()
            .get::<WantsHtml>()
            .is_some_and(|w| w.0);

        if !wants_html {
            return response;
        }

        let ctx = Self::build_error_context(error, &response, self.is_dev);
        let mut html_body =
            error_pages::render_error_page(self.renderer.as_ref(), error.status, &ctx)
                .into_string();

        if self.is_dev {
            self.inject_dev_badge(&mut html_body, error, &ctx, &response);
        }

        Self::build_html_response(error, html_body)
    }
}

/// Marker stored in response extensions to indicate the original request
/// preferred HTML responses.
#[derive(Clone, Debug)]
pub struct WantsHtml(pub bool);

/// Request context stored in response extensions for the error page filter.
#[derive(Clone, Debug)]
pub struct ErrorPageRequestContext {
    pub uri: axum::http::Uri,
    pub request_id: Option<String>,
    pub query: Option<String>,
    pub headers: Option<axum::http::HeaderMap>,
    /// HTTP method (e.g. "GET").
    pub method: Option<String>,
    /// Matched route pattern (e.g. `/posts/{id}`), set by `DevRouteInfoLayer` in dev mode.
    pub matched_path: Option<String>,
    /// Scrubbed cookies parsed from the Cookie header.
    pub cookies: Option<serde_json::Value>,
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
        let uri = req.uri().clone();
        let request_id = req
            .extensions()
            .get::<crate::middleware::RequestId>()
            .map(std::string::ToString::to_string);

        let query = uri.query().map(str::to_owned);
        let headers = Some(req.headers().clone());
        let method = Some(req.method().to_string());

        ErrorPageContextFuture {
            inner: self.inner.call(req),
            wants_html,
            uri,
            request_id,
            query,
            headers,
            method,
        }
    }
}

pin_project_lite::pin_project! {
    pub struct ErrorPageContextFuture<F> {
        #[pin]
        inner: F,
        wants_html: bool,
        uri: axum::http::Uri,
        request_id: Option<String>,
        query: Option<String>,
        headers: Option<axum::http::HeaderMap>,
        method: Option<String>,
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
                let request_id = this.request_id.clone().or_else(|| {
                    response
                        .headers()
                        .get("x-request-id")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned)
                });
                let matched_path = response
                    .extensions()
                    .get::<crate::middleware::dev::DevMatchedPath>()
                    .map(|m| m.0.clone());
                let cookies = this
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(axum::http::header::COOKIE))
                    .and_then(|v| v.to_str().ok())
                    .map(parse_cookie_header);
                response.extensions_mut().insert(ErrorPageRequestContext {
                    uri: this.uri.clone(),
                    request_id,
                    query: this.query.clone(),
                    headers: this.headers.clone(),
                    method: this.method.clone(),
                    matched_path,
                    cookies,
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
    accept_prefers_html(req.headers())
}

/// Check whether an Accept header prefers an HTML response over JSON.
pub fn accept_prefers_html(headers: &axum::http::HeaderMap) -> bool {
    let accept = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // If no Accept header, default to JSON (API-first).
    if accept.is_empty() {
        return false;
    }

    let mut html: Option<(f32, usize)> = None;
    let mut json: Option<(f32, usize)> = None;
    let mut wildcard: Option<(f32, usize)> = None;

    for (index, raw_part) in accept.split(',').enumerate() {
        let part = raw_part.trim();
        if part.is_empty() {
            continue;
        }

        let mut mime = "";
        let mut q = 1.0_f32;

        for (i, segment) in part.split(';').enumerate() {
            let segment = segment.trim();
            if i == 0 {
                mime = segment;
                continue;
            }

            if let Some(value) = segment.strip_prefix("q=")
                && let Ok(parsed) = value.trim().parse::<f32>()
            {
                q = parsed.clamp(0.0, 1.0);
            }
        }
        if q <= 0.0 {
            continue;
        }

        match mime {
            "text/html" if html.is_none_or(|(existing_q, _)| q > existing_q) => {
                html = Some((q, index));
            }
            "application/json" | "application/problem+json"
                if json.is_none_or(|(existing_q, _)| q > existing_q) =>
            {
                json = Some((q, index));
            }
            "*/*" if wildcard.is_none_or(|(existing_q, _)| q > existing_q) => {
                wildcard = Some((q, index));
            }
            _ => {}
        }
    }

    match (html, json, wildcard) {
        (Some((hq, hidx)), Some((jq, jidx)), _) => {
            if (hq - jq).abs() < f32::EPSILON {
                hidx < jidx
            } else {
                hq > jq
            }
        }
        (Some(_), None, _) | (None, None, Some(_)) => true,
        (None, Some(_), _) | (None, None, None) => false,
    }
}

/// Parse `Cookie: name=value; name2=value2` into a JSON object.
///
/// Returns an object where each cookie name maps to its (possibly filtered) value.
fn parse_cookie_header(raw: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((name, value)) = pair.split_once('=') {
            map.insert(name.trim().to_owned(), serde_json::Value::String(value.trim().to_owned()));
        } else if !pair.is_empty() {
            map.insert(pair.to_owned(), serde_json::Value::String(String::new()));
        }
    }
    serde_json::Value::Object(map)
}

fn scrub_headers(
    headers: &axum::http::HeaderMap,
    parameter_filter: &crate::log::filter::ParameterFilter,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        let key = name.as_str().to_owned();
        let val = value.to_str().unwrap_or("<non-utf8>").to_owned();
        map.insert(key, serde_json::Value::String(val));
    }
    parameter_filter.scrub_json(&serde_json::Value::Object(map))
}

/// 404 fallback handler for unmatched routes.
///
/// This is mounted as the router's fallback so unmatched routes get proper
/// error pages instead of Axum's default plain-text "Not Found".
pub async fn fallback_404_handler(method: axum::http::Method, uri: axum::http::Uri) -> Response {
    if matches!(method, axum::http::Method::GET | axum::http::Method::HEAD)
        && uri.path() == crate::router::DEFAULT_FAVICON_PATH
    {
        return axum::http::StatusCode::NO_CONTENT.into_response();
    }

    crate::error::AutumnError::not_found_msg(format!("No route matches {}", uri.path()))
        .into_response()
}

impl ErrorPageFilter {
    fn build_error_context(
        error: &AutumnErrorInfo,
        response: &Response,
        is_dev: bool,
    ) -> ErrorContext {
        let request_id = response
            .extensions()
            .get::<ErrorPageRequestContext>()
            .and_then(|ctx| ctx.request_id.clone());

        let path = response
            .extensions()
            .get::<ErrorPageRequestContext>()
            .map(|ctx| ctx.uri.path().to_string())
            .unwrap_or_default();

        ErrorContext {
            status: error.status,
            message: error.message.clone(),
            path,
            request_id,
            details: error.details.clone(),
            is_dev,
        }
    }

    fn inject_dev_badge(
        &self,
        html_body: &mut String,
        error: &AutumnErrorInfo,
        ctx: &ErrorContext,
        response: &Response,
    ) {
        let req_ctx = response.extensions().get::<ErrorPageRequestContext>();

        let stack_frames = error
            .backtrace_string
            .as_deref()
            .map(|bt| {
                crate::error_pages::source::parse_backtrace_string(bt, 24)
            })
            .unwrap_or_default();

        let raw_cookies = req_ctx
            .and_then(|c| c.cookies.as_ref())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let cookies = self.parameter_filter.scrub_json(&raw_cookies);

        let badge_ctx = DevBadgeContext {
            status_code: error.status.as_u16(),
            status_reason: error
                .status
                .canonical_reason()
                .unwrap_or("Error")
                .to_string(),
            message: error.message.clone(),
            path: ctx.path.clone(),
            request_id: ctx.request_id.clone(),
            source_location: None,
            query: req_ctx.and_then(|c| c.query.clone()),
            headers: req_ctx
                .and_then(|c| c.headers.as_ref())
                .map_or_else(
                    || serde_json::json!({}),
                    |h| scrub_headers(h, &self.parameter_filter),
                ),
            method: req_ctx.and_then(|c| c.method.clone()),
            route_pattern: req_ctx.and_then(|c| c.matched_path.clone()),
            path_params: serde_json::json!({}),
            cookies,
            stack_frames,
            sql_queries: Vec::new(),
        };
        let badge = dev_badge::dev_error_badge_html(&badge_ctx).into_string();
        if let Some(pos) = html_body.rfind("</body>") {
            html_body.insert_str(pos, &badge);
        } else {
            html_body.push_str(&badge);
        }
    }

    fn build_html_response(error: &AutumnErrorInfo, html_body: String) -> Response {
        let content_length = html_body.len();

        let mut resp = (
            error.status,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html_body,
        )
            .into_response();

        // Re-attach error info so downstream filters still see it
        resp.extensions_mut().insert(error.clone());

        // Ensure content-length is set correctly, as middleware might otherwise
        // drop it in some environments like fallback routes.
        resp.headers_mut().insert(
            axum::http::header::CONTENT_LENGTH,
            axum::http::HeaderValue::from(content_length),
        );

        resp
    }
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

    #[test]
    fn prefers_json_when_json_has_higher_q() {
        let req = Request::builder()
            .header("accept", "text/html;q=0.4, application/json;q=0.9")
            .body(Body::empty())
            .unwrap();
        assert!(!accepts_html(&req));
    }

    #[test]
    fn prefers_problem_json_when_problem_json_has_higher_q() {
        let req = Request::builder()
            .header("accept", "application/problem+json, text/html;q=0.1")
            .body(Body::empty())
            .unwrap();
        assert!(!accepts_html(&req));
    }

    #[test]
    fn prefers_html_when_html_has_higher_q() {
        let req = Request::builder()
            .header("accept", "application/json;q=0.3, text/html;q=0.8")
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
        let error_page_filter = ErrorPageFilter {
            renderer,
            is_dev,
            parameter_filter: crate::log::filter::ParameterFilter::default(),
        };
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

        let content_length = resp
            .headers()
            .get("content-length")
            .expect("Content-Length header should be set")
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(content_length, body.len());
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
            ct.contains("application/problem+json"),
            "JSON API should get Problem Details response, got: {ct}"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], 404);
        assert_eq!(json["code"], "autumn.not_found");
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
            parameter_filter: crate::log::filter::ParameterFilter::default(),
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
    async fn custom_renderer_is_ignored_for_json_requests() {
        use crate::error_pages::{ErrorContext, ErrorPageRenderer};
        use maud::{Markup, html};

        struct LoudCustomPages;
        impl ErrorPageRenderer for LoudCustomPages {
            fn render_error(&self, ctx: &ErrorContext) -> Markup {
                html! {
                    h1 { "LOUD CUSTOM " (ctx.status.as_u16()) }
                }
            }
        }

        let renderer: crate::error_pages::SharedRenderer = Arc::new(LoudCustomPages);
        let error_page_filter = ErrorPageFilter {
            renderer,
            is_dev: false,
            parameter_filter: crate::log::filter::ParameterFilter::default(),
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
            .expect("content-type should be present")
            .to_str()
            .expect("content-type should be valid UTF-8");
        assert!(
            ct.contains("application/problem+json"),
            "JSON requests should still get Problem Details, got: {ct}"
        );

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !body_str.contains("LOUD CUSTOM 404"),
            "custom HTML renderer should not run for JSON requests, got: {body_str}"
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

    #[tokio::test]
    async fn fallback_404_handler_creates_correct_error() {
        let uri = axum::http::Uri::from_static("/some/unknown/path");
        let response = fallback_404_handler(axum::http::Method::GET, uri).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("No route matches /some/unknown/path"));
    }

    #[tokio::test]
    async fn fallback_404_handler_ignores_query_params() {
        let uri = axum::http::Uri::from_static("/search?q=rust&sort=desc");
        let response = fallback_404_handler(axum::http::Method::GET, uri).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("No route matches /search"));
    }

    #[tokio::test]
    async fn fallback_404_handler_with_root_path() {
        let uri = axum::http::Uri::from_static("/");
        let response = fallback_404_handler(axum::http::Method::GET, uri).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("No route matches /"));
    }

    #[tokio::test]
    async fn fallback_404_handler_returns_empty_no_content_for_favicon_get() {
        let response = fallback_404_handler(
            axum::http::Method::GET,
            axum::http::Uri::from_static(crate::router::DEFAULT_FAVICON_PATH),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn fallback_404_handler_returns_empty_no_content_for_favicon_head() {
        let response = fallback_404_handler(
            axum::http::Method::HEAD,
            axum::http::Uri::from_static(crate::router::DEFAULT_FAVICON_PATH),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn dev_badge_scrubs_sensitive_headers_on_form_post() {
        let app = test_router_with_error_pages(true);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope?debug=true")
                .header("accept", "text/html")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("authorization", "Bearer super-secret-token")
                .body(Body::from("password=hunter2&email=user@example.com"))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(body_str.contains("authorization"));
        assert!(body_str.contains("[FILTERED]"));
        assert!(body_str.contains("Query"));
    }

    #[tokio::test]
    async fn dev_badge_uses_configured_custom_filter_parameters() {
        let renderer = error_pages::default_renderer();
        let error_page_filter = ErrorPageFilter {
            renderer,
            is_dev: true,
            parameter_filter: crate::log::filter::ParameterFilter::new(&["pin".to_owned()], &[]),
        };
        let filters: Vec<Arc<dyn crate::middleware::ExceptionFilter>> =
            vec![Arc::new(error_page_filter)];

        let app = Router::new()
            .fallback(fallback_404_handler)
            .layer(ErrorPageContextLayer)
            .layer(ExceptionFilterLayer::new(filters));

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope")
                .header("accept", "text/html")
                .header("pin", "1234")
                .body(Body::from("x=1"))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(body_str.contains("pin"));
        assert!(body_str.contains("[FILTERED]"));
    }

    #[tokio::test]
    async fn fallback_404_handler_keeps_non_get_favicon_requests_as_not_found() {
        let response = fallback_404_handler(
            axum::http::Method::POST,
            axum::http::Uri::from_static(crate::router::DEFAULT_FAVICON_PATH),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
