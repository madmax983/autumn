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
    /// Raw path parameters (e.g. `{"id": "42"}`), captured in dev mode.
    pub path_params: Option<serde_json::Value>,
    /// SQL queries recorded during this request by `InspectorLayer` (dev only).
    pub sql_queries: Vec<crate::inspector::QueryRecord>,
    /// Buffered request body bytes (dev mode only, capped at 64 KB).
    pub body_bytes: Option<bytes::Bytes>,
    /// `Content-Type` header value at the time of buffering (dev mode only).
    pub content_type: Option<String>,
    /// True when the body exceeded the capture limit and was truncated.
    pub body_truncated: bool,
}

/// Max bytes captured from the request body for the dev overlay.
/// Bodies larger than this will show a truncation notice.
const BODY_CAPTURE_LIMIT: usize = 64 * 1024;

/// Tower layer that annotates requests with Accept header preference
/// so the error page filter knows whether to render HTML or pass through JSON.
///
/// This layer runs before the exception filter and stores [`WantsHtml`]
/// and [`ErrorPageRequestContext`] in the response extensions.
#[derive(Clone)]
pub struct ErrorPageContextLayer {
    pub is_dev: bool,
}

impl<S> tower::Layer<S> for ErrorPageContextLayer {
    type Service = ErrorPageContextService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ErrorPageContextService {
            inner,
            is_dev: self.is_dev,
        }
    }
}

#[derive(Clone)]
pub struct ErrorPageContextService<S> {
    inner: S,
    is_dev: bool,
}

impl<S> tower::Service<axum::http::Request<axum::body::Body>> for ErrorPageContextService<S>
where
    S: tower::Service<axum::http::Request<axum::body::Body>, Response = Response>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: axum::http::Request<axum::body::Body>) -> Self::Future {
        let wants_html = accepts_html(&req);
        let uri = req.uri().clone();
        let request_id = req
            .extensions()
            .get::<crate::middleware::RequestId>()
            .map(std::string::ToString::to_string);

        let query = uri.query().map(str::to_owned);
        let headers = Some(req.headers().clone());
        let method = Some(req.method().to_string());
        let is_dev = self.is_dev;
        let captures_body = is_dev && should_capture_body(method.as_deref());
        let content_type = if captures_body {
            req.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        } else {
            None
        };

        // Use clone-and-replace so we can move inner into the async block while
        // still having called poll_ready on the original.
        let cloned = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, cloned);

        Box::pin(async move {
            let (req, body_bytes, body_truncated) = if captures_body {
                // Check Content-Length first to avoid consuming the body when it's
                // clearly over the cap (preserving the stream for the inner service).
                let declared_len = req
                    .headers()
                    .get(axum::http::header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<usize>().ok());

                if declared_len.is_some_and(|len| len > BODY_CAPTURE_LIMIT) {
                    // Body is known to be too large — don't consume it at all.
                    (req, None, true)
                } else {
                    let (parts, body) = req.into_parts();
                    // Read up to BODY_CAPTURE_LIMIT + 1 bytes to detect truncation.
                    // to_bytes returns Err if the stream exceeds the limit; treat
                    // that as a truncated body and pass an empty body downstream
                    // (better than silently truncating mid-stream with no signal).
                    if let Ok(bytes) = axum::body::to_bytes(body, BODY_CAPTURE_LIMIT + 1).await {
                        let truncated = bytes.len() > BODY_CAPTURE_LIMIT;
                        let captured = bytes.slice(..bytes.len().min(BODY_CAPTURE_LIMIT));
                        let new_body = axum::body::Body::from(captured.clone());
                        let req = axum::http::Request::from_parts(parts, new_body);
                        (req, Some(captured), truncated)
                    } else {
                        // Body exceeded the limit mid-stream; forward empty body
                        // and show truncation notice. The handler will see an
                        // empty body in dev mode, which is acceptable.
                        let req = axum::http::Request::from_parts(parts, axum::body::Body::empty());
                        (req, None, true)
                    }
                }
            } else {
                (req, None, false)
            };

            let mut response = inner.call(req).await?;

            response.extensions_mut().insert(WantsHtml(wants_html));
            let request_id = request_id.or_else(|| {
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
            let path_params = matched_path
                .as_deref()
                .map(|pattern| extract_path_params(pattern, uri.path()));
            let cookies = headers
                .as_ref()
                .and_then(|h| h.get(axum::http::header::COOKIE))
                .and_then(|v| v.to_str().ok())
                .map(parse_cookie_header);
            let sql_queries = response
                .extensions()
                .get::<crate::inspector::RequestQueryList>()
                .map(crate::inspector::RequestQueryList::snapshot)
                .unwrap_or_default();
            response.extensions_mut().insert(ErrorPageRequestContext {
                uri,
                request_id,
                query,
                headers,
                method,
                matched_path,
                cookies,
                path_params,
                sql_queries,
                body_bytes,
                content_type,
                body_truncated,
            });
            Ok(response)
        })
    }
}

/// Returns true for HTTP methods that typically carry a request body.
fn should_capture_body(method: Option<&str>) -> bool {
    matches!(method, Some("POST" | "PUT" | "PATCH"))
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

/// Derive path parameters by matching URI segments against a route pattern.
///
/// Handles both `{name}` (Axum/Autumn default) and `:name` (legacy) capture
/// syntax. Returns an empty object when segment counts differ.
fn extract_path_params(pattern: &str, uri_path: &str) -> serde_json::Value {
    let pat_segs: Vec<&str> = pattern.split('/').collect();
    let uri_segs: Vec<&str> = uri_path.split('/').collect();
    let mut map = serde_json::Map::new();
    if pat_segs.len() == uri_segs.len() {
        for (pat, val) in pat_segs.iter().zip(uri_segs.iter()) {
            let name = pat
                .strip_prefix(':')
                .or_else(|| pat.strip_prefix('{').and_then(|s| s.strip_suffix('}')));
            if let Some(name) = name {
                map.insert(
                    name.to_owned(),
                    serde_json::Value::String((*val).to_owned()),
                );
            }
        }
    }
    serde_json::Value::Object(map)
}

/// Convert an inspector [`QueryRecord`] to the overlay's [`SqlQueryInfo`].
fn query_record_to_sql_info(
    r: &crate::inspector::QueryRecord,
) -> crate::error_pages::dev_badge::SqlQueryInfo {
    crate::error_pages::dev_badge::SqlQueryInfo {
        statement: r.sql.clone(),
        bind_count: r.params.len(),
        #[allow(clippy::cast_precision_loss)]
        duration_ms: r.elapsed_ms as f64,
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
            map.insert(
                name.trim().to_owned(),
                serde_json::Value::String(value.trim().to_owned()),
            );
        } else if !pair.is_empty() {
            map.insert(pair.to_owned(), serde_json::Value::String(String::new()));
        }
    }
    serde_json::Value::Object(map)
}

/// Parse request body bytes into a JSON value for the dev overlay.
///
/// Understands `application/json` and `application/x-www-form-urlencoded`.
/// Returns `None` for empty bodies or unrecognised content types.
fn parse_body_preview(
    bytes: &bytes::Bytes,
    content_type: Option<&str>,
) -> Option<serde_json::Value> {
    if bytes.is_empty() {
        return None;
    }
    let ct = content_type.unwrap_or("");
    if ct.contains("application/json") {
        serde_json::from_slice(bytes).ok()
    } else if ct.contains("application/x-www-form-urlencoded") {
        let map: std::collections::HashMap<String, String> =
            serde_urlencoded::from_bytes(bytes).ok()?;
        if map.is_empty() {
            None
        } else {
            Some(serde_json::to_value(map).unwrap_or_else(|_| serde_json::json!({})))
        }
    } else {
        None
    }
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
            .map(|bt| crate::error_pages::source::parse_backtrace_string(bt, 24))
            .unwrap_or_default();

        let raw_cookies = req_ctx
            .and_then(|c| c.cookies.as_ref())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let cookies = self.parameter_filter.scrub_json(&raw_cookies);

        let body_preview = req_ctx.and_then(|c| {
            if c.body_truncated {
                return Some(serde_json::Value::String(
                    "[body truncated: exceeds 64 KB limit]".to_owned(),
                ));
            }
            let bytes = c.body_bytes.as_ref()?;
            let parsed = parse_body_preview(bytes, c.content_type.as_deref())?;
            Some(self.parameter_filter.scrub_json(&parsed))
        });

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
            headers: req_ctx.and_then(|c| c.headers.as_ref()).map_or_else(
                || serde_json::json!({}),
                |h| scrub_headers(h, &self.parameter_filter),
            ),
            method: req_ctx.and_then(|c| c.method.clone()),
            route_pattern: req_ctx.and_then(|c| c.matched_path.clone()),
            path_params: req_ctx.and_then(|c| c.path_params.as_ref()).map_or_else(
                || serde_json::json!({}),
                |p| self.parameter_filter.scrub_json(p),
            ),
            cookies,
            stack_frames,
            sql_queries: req_ctx
                .map(|c| c.sql_queries.iter().map(query_record_to_sql_info).collect())
                .unwrap_or_default(),
            body_preview,
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
            .layer(ErrorPageContextLayer { is_dev })
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
            .layer(ErrorPageContextLayer { is_dev: false })
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
            .layer(ErrorPageContextLayer { is_dev: false })
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
            .layer(ErrorPageContextLayer { is_dev: true })
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

    #[test]
    fn extract_path_params_brace_syntax() {
        let v = super::extract_path_params("/posts/{id}/comments/{cid}", "/posts/42/comments/7");
        assert_eq!(v["id"], "42");
        assert_eq!(v["cid"], "7");
    }

    #[test]
    fn extract_path_params_colon_syntax() {
        let v = super::extract_path_params("/posts/:id/comments/:cid", "/posts/42/comments/7");
        assert_eq!(v["id"], "42");
        assert_eq!(v["cid"], "7");
    }

    #[test]
    fn extract_path_params_static_route_returns_empty() {
        let v = super::extract_path_params("/posts", "/posts");
        assert_eq!(v.as_object().unwrap().len(), 0);
    }

    #[test]
    fn extract_path_params_segment_count_mismatch_returns_empty() {
        let v = super::extract_path_params("/posts/{id}", "/posts/42/extra");
        assert_eq!(
            v.as_object().unwrap().len(),
            0,
            "mismatched segments should yield empty map"
        );
    }

    #[test]
    fn extract_path_params_root_wildcard_returns_empty_for_multisegment_uri() {
        // A wildcard capture like `{*rest}` spans multiple URI segments but the
        // naive segment-split produces a length mismatch; we should return empty
        // rather than crashing or silently dropping segments.
        let v = super::extract_path_params("/files/{*rest}", "/files/foo/bar/baz");
        assert_eq!(
            v.as_object().unwrap().len(),
            0,
            "wildcard glob should yield empty map"
        );
    }

    // ── AC2: path param values scrubbed via ParameterFilter ──────────────────

    #[test]
    fn extract_path_params_scrubbed_by_parameter_filter() {
        // A route like /reset/{token} would expose a sensitive value; the
        // ParameterFilter should mask it before it appears in the overlay.
        let raw = super::extract_path_params("/reset/{token}", "/reset/abc123secret");
        let filter = crate::log::filter::ParameterFilter::default();
        let scrubbed = filter.scrub_json(&raw);
        assert_eq!(
            scrubbed["token"], "[FILTERED]",
            "token param value should be scrubbed by the default filter"
        );
    }

    // ── AC3: Path Params section hidden when route has no path params ─────────

    #[tokio::test]
    async fn dev_badge_hides_path_params_section_for_static_route() {
        // A route with no `{param}` segments must NOT render the Path Params
        // section in the overlay.
        let app = test_router_with_error_pages_and_matched_path(true);
        let resp = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .uri("/fail")
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
            !body_str.contains("Path Params"),
            "static route overlay must not include Path Params section, got:\n{body_str}"
        );
    }

    // ── AC1: overlay shows parsed path params ────────────────────────────────

    #[tokio::test]
    async fn dev_badge_shows_path_params_in_overlay() {
        // A route with a `{id}` param must render the parsed value in the overlay.
        let app = test_router_with_error_pages_and_matched_path(true);
        let resp = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .uri("/items/99")
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
            body_str.contains("Path Params"),
            "overlay must include Path Params section for parametric route"
        );
        assert!(
            body_str.contains("99"),
            "overlay must show extracted path param value"
        );
    }

    // ── AC2 integration: sensitive path param scrubbed in overlay ────────────

    #[tokio::test]
    async fn dev_badge_scrubs_sensitive_path_param_in_overlay() {
        // Route is /tokens/{token}; "token" is a DEFAULT_FILTER_KEYS entry.
        // The Path Params table must show [FILTERED], not the raw value.
        // Note: the raw URI path still appears in the "Path" row — only the
        // *parsed* params map is scrubbed by ParameterFilter.
        let app = test_router_with_error_pages_and_matched_path(true);
        let resp = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .uri("/tokens/supersecret")
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
            body_str.contains("Path Params"),
            "overlay must show Path Params section even when value is filtered"
        );
        assert!(
            body_str.contains("[FILTERED]"),
            "sensitive path param value must be scrubbed in the Path Params map"
        );
    }

    /// Build a test router that additionally applies `capture_matched_path_middleware`
    /// (simulating dev profile) and includes routes with path parameters.
    fn test_router_with_error_pages_and_matched_path(is_dev: bool) -> axum::Router {
        use crate::middleware::dev::capture_matched_path_middleware;
        use axum::routing::get;

        let renderer = error_pages::default_renderer();
        let error_page_filter = ErrorPageFilter {
            renderer,
            is_dev,
            parameter_filter: crate::log::filter::ParameterFilter::default(),
        };
        let filters: Vec<Arc<dyn crate::middleware::ExceptionFilter>> =
            vec![Arc::new(error_page_filter)];

        axum::Router::new()
            .route(
                "/fail",
                get(|| async { Err::<String, AutumnError>(AutumnError::not_found_msg("gone")) }),
            )
            .route(
                "/items/{id}",
                get(|| async {
                    Err::<String, AutumnError>(AutumnError::not_found_msg("item not found"))
                }),
            )
            .route(
                "/tokens/{token}",
                get(|| async {
                    Err::<String, AutumnError>(AutumnError::not_found_msg("token not found"))
                }),
            )
            .route_layer(axum::middleware::from_fn(capture_matched_path_middleware))
            .fallback(fallback_404_handler)
            .layer(ErrorPageContextLayer)
            .layer(ExceptionFilterLayer::new(filters))
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

    // ── Body capture tests (issue #1081) ────────────────────────────────────

    #[tokio::test]
    async fn dev_badge_shows_form_body_in_dev_mode() {
        let app = test_router_with_error_pages(true);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope")
                .header("accept", "text/html")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=alice&age=30"))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(
            body_str.contains("Body"),
            "dev overlay should show a Body section for form POST, got:\n{body_str}"
        );
        assert!(
            body_str.contains("alice"),
            "dev overlay should show form field value, got:\n{body_str}"
        );
    }

    #[tokio::test]
    async fn dev_badge_shows_json_body_in_dev_mode() {
        let app = test_router_with_error_pages(true);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope")
                .header("accept", "text/html")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"user":"bob","score":42}"#))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(
            body_str.contains("Body"),
            "dev overlay should show a Body section for JSON POST"
        );
        assert!(
            body_str.contains("bob"),
            "dev overlay should show JSON field value"
        );
    }

    #[tokio::test]
    async fn dev_badge_scrubs_sensitive_form_body_fields() {
        let app = test_router_with_error_pages(true);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope")
                .header("accept", "text/html")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=alice&password=secret123"))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(
            body_str.contains("password"),
            "should show the password key"
        );
        assert!(
            !body_str.contains("secret123"),
            "raw password value must be filtered out"
        );
        assert!(
            body_str.contains("[FILTERED]"),
            "filtered field should show [FILTERED]"
        );
    }

    #[tokio::test]
    async fn dev_badge_scrubs_sensitive_json_body_fields() {
        let app = test_router_with_error_pages(true);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope")
                .header("accept", "text/html")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"user":"alice","token":"abc123"}"#))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(body_str.contains("token"), "should show the token key");
        assert!(
            !body_str.contains("abc123"),
            "raw token value must be filtered out"
        );
        assert!(body_str.contains("[FILTERED]"), "should show [FILTERED]");
    }

    #[tokio::test]
    async fn dev_badge_does_not_show_body_in_prod_mode() {
        let app = test_router_with_error_pages(false);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope")
                .header("accept", "text/html")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=alice&age=30"))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        // In prod mode there is no dev badge at all, so no Body section
        assert!(
            !body_str.contains("autumn-dev-error-badge"),
            "prod mode must not include the dev badge"
        );
    }

    #[tokio::test]
    async fn dev_badge_does_not_show_body_section_for_get() {
        let app = test_router_with_error_pages(true);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("GET")
                .uri("/nope")
                .header("accept", "text/html")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        // The overlay should exist but not have a Body section for GET
        assert!(
            body_str.contains("autumn-dev-error-badge"),
            "dev badge should appear on GET errors"
        );
        // No "Body" label in the overlay for a GET request
        assert!(
            !body_str.contains(">Body<"),
            "GET requests should not show a Body section"
        );
    }

    #[tokio::test]
    async fn dev_badge_truncates_oversized_body() {
        let app = test_router_with_error_pages(true);

        // Build a body larger than the 64 KB cap
        let large_body = "x=".to_owned() + &"a".repeat(70 * 1024);

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method("POST")
                .uri("/nope")
                .header("accept", "text/html")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(large_body))
                .expect("request"),
        )
        .await
        .expect("response");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(
            body_str.contains("truncated"),
            "oversized body should show a truncation notice, got:\n{body_str}"
        );
    }
}
