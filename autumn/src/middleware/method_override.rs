//! HTML form method-override middleware.
//!
//! Native browser HTML forms can only submit `GET` or `POST`. Autumn lets
//! plain `<form method="post">` submissions target `PUT`, `PATCH`, and
//! `DELETE` routes by carrying a hidden `_method` form field whose value
//! names the effective HTTP method.
//!
//! # How it works
//!
//! On every `POST` request whose body is a same-origin form submission
//! (content-type `application/x-www-form-urlencoded`), the middleware
//! looks for the configured override field (default `_method`). If
//! present and the value is one of the documented target methods
//! (`PUT`, `PATCH`, `DELETE`, case-insensitive), the request's HTTP
//! method is rewritten in place before route matching. The body is
//! preserved exactly so downstream extractors (including CSRF
//! validation) read it unchanged.
//!
//! # Same-origin requirement
//!
//! The override is strictly a convention for browser HTML forms
//! originating from the same origin. The layer enforces this rather
//! than relying solely on CSRF, because a route declared as `#[delete]`
//! would otherwise become reachable from any cross-origin form when
//! CSRF protection is disabled or the path is CSRF-exempt — a
//! reachability change browsers' same-origin policy would normally
//! prevent for direct `DELETE` requests.
//!
//! The check uses, in order:
//!
//! 1. `Sec-Fetch-Site` (sent by all browsers since ~2020). Allowed
//!    values: `same-origin`, `same-site`, `none`. `cross-site` is
//!    rejected.
//! 2. Fallback when `Sec-Fetch-Site` is absent: `Origin` is compared
//!    host-for-host with `Host`. A mismatch is rejected.
//! 3. If both signals are absent, the override is **not** applied —
//!    the request is forwarded as the original `POST` so route
//!    matching rejects it on its own (fail closed).
//!
//! # Failure modes
//!
//! - An unrecognised override value (e.g. `_method=BREW`) is rejected
//!   with `400 Bad Request` before reaching the handler.
//! - The override only acts on `POST` requests with a form-style body.
//!   Headers like `X-HTTP-Method-Override` are intentionally **not**
//!   honoured: this convention is documented for browser HTML forms
//!   only, not for arbitrary REST tunneling.
//!
//! ## How the rejection flows through middleware
//!
//! The layer is wrapped outside the router so it can rewrite the HTTP
//! method **before route matching**. Returning a `400`/`413` directly
//! from that outer wrapper would bypass every framework layer applied
//! via `Router::layer` — security headers, request IDs, metrics,
//! exception filter, error-page renderer, and so on. Instead, the
//! layer stamps a [`MethodOverrideRejection`] extension on the request
//! and forwards it down the stack untouched. A companion middleware,
//! [`method_override_rejection_filter`], is applied as an inner
//! `Router::layer` so the rejection is converted into a `400`/`413`
//! response **inside** the framework's response middleware stack. The
//! result is the same status the user sees, but with security headers,
//! request IDs, metrics, and error-page rendering all applied.
//!
//! # Interaction with CSRF
//!
//! The CSRF layer wraps this middleware on the outside, so CSRF still
//! validates the original `POST` request — meaning an overridden
//! `DELETE` submission without a valid `_csrf` token is rejected
//! exactly like any other mutating POST.
//!
//! # Examples
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::security::CsrfToken;
//!
//! #[get("/posts/{id}/edit")]
//! async fn edit_form(csrf: CsrfToken) -> Markup {
//!     html! {
//!         form method="post" action="/posts/42" {
//!             input type="hidden" name="_method" value="PUT";
//!             input type="hidden" name="_csrf" value=(csrf.token());
//!             input type="text" name="title";
//!             button { "Update" }
//!         }
//!     }
//! }
//!
//! #[put("/posts/{id}")]
//! async fn update(/* extractors */) -> Markup { html! { "updated" } }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::http::{Request, Response, StatusCode};
use tower::{Layer, Service};

/// Default form field name for the method override.
pub const DEFAULT_METHOD_OVERRIDE_FIELD: &str = "_method";

/// Maximum body size (bytes) considered when parsing the override field.
///
/// Mirrors [`crate::security::csrf`] which uses the same 2 MiB cap for
/// peeking at form-urlencoded bodies. The body itself is unchanged.
const MAX_BODY_SCAN_BYTES: usize = 2 * 1024 * 1024;

/// Configuration for the method-override middleware.
#[derive(Debug, Clone)]
pub struct MethodOverrideConfig {
    /// Form field name to read the override value from.
    pub field_name: String,
}

impl Default for MethodOverrideConfig {
    fn default() -> Self {
        Self {
            field_name: DEFAULT_METHOD_OVERRIDE_FIELD.to_owned(),
        }
    }
}

/// Marker extension inserted on requests whose HTTP method was rewritten
/// by the override middleware. Useful for logs and instrumentation.
#[derive(Clone, Debug)]
pub struct OverriddenMethod {
    /// The transport method the browser actually used (always `POST`).
    pub transport: http::Method,
    /// The effective method the request was rewritten to.
    pub effective: http::Method,
}

/// Reason the override middleware needs to fail the request.
///
/// Stamped on the request extensions by the outer Tower layer so the
/// inner [`method_override_rejection_filter`] can convert it into a
/// proper `400`/`413` response that flows through the framework's
/// response middleware stack (security headers, request IDs, error
/// pages, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodOverrideRejection {
    /// `_method` carried a value that wasn't `PUT`, `PATCH`, or `DELETE`
    /// (case-insensitive). Rendered as `400 Bad Request`.
    InvalidValue,
    /// Form body was too large to buffer for override scanning and the
    /// bytes have already been consumed. Rendered as `413 Payload Too
    /// Large`.
    BodyTooLarge,
}

/// Inner middleware that converts a [`MethodOverrideRejection`] into a
/// `400`/`413` response.
///
/// Applied via `Router::layer` so the rejection is rendered inside the
/// per-route layer chain. Running here means the response inherits the
/// framework's outer middleware (security headers, request IDs,
/// metrics, error-page filter, exception filter) rather than bypassing
/// them.
///
/// The filter is a no-op when no rejection extension is present, so it
/// is safe to apply to every route in the router.
pub async fn method_override_rejection_filter(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use crate::middleware::exception_filter::AutumnErrorInfo;
    use axum::response::IntoResponse;

    if let Some(rejection) = request
        .extensions()
        .get::<MethodOverrideRejection>()
        .copied()
    {
        let (status, message) = match rejection {
            MethodOverrideRejection::InvalidValue => (
                StatusCode::BAD_REQUEST,
                "Invalid method override value: must be PUT, PATCH, or DELETE.",
            ),
            MethodOverrideRejection::BodyTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "Form body too large for method-override scanning.",
            ),
        };
        let mut response = (
            status,
            [(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            message,
        )
            .into_response();
        // Surface this as a framework error so the exception filter
        // chain (problem-details normalization, custom HTML error-page
        // rendering) processes it the same as any handler-generated
        // error response. Without this extension, `ExceptionFilterFuture`
        // treats the response as pre-formed and skips renegotiation.
        response.extensions_mut().insert(AutumnErrorInfo {
            status,
            message: message.to_owned(),
            details: None,
            problem_type: None,
        });
        return response;
    }
    next.run(request).await
}

/// Tower [`Layer`] that applies the HTML form method override convention.
#[derive(Clone, Debug)]
pub struct MethodOverrideLayer {
    config: Arc<MethodOverrideConfig>,
}

impl MethodOverrideLayer {
    /// Construct a layer with the default field name (`_method`).
    #[must_use]
    pub fn new() -> Self {
        Self::from_config(MethodOverrideConfig::default())
    }

    /// Construct a layer with the given configuration.
    #[must_use]
    pub fn from_config(config: MethodOverrideConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

impl Default for MethodOverrideLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for MethodOverrideLayer {
    type Service = MethodOverrideService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MethodOverrideService {
            inner,
            config: Arc::clone(&self.config),
        }
    }
}

/// Tower [`Service`] produced by [`MethodOverrideLayer`].
#[derive(Clone, Debug)]
pub struct MethodOverrideService<S> {
    inner: S,
    config: Arc<MethodOverrideConfig>,
}

/// Returns `Some(method)` when `value` is a recognised override target.
fn parse_override_value(value: &str) -> Option<http::Method> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("PUT") {
        Some(http::Method::PUT)
    } else if trimmed.eq_ignore_ascii_case("PATCH") {
        Some(http::Method::PATCH)
    } else if trimmed.eq_ignore_ascii_case("DELETE") {
        Some(http::Method::DELETE)
    } else {
        None
    }
}

#[derive(Debug)]
enum OverrideOutcome {
    /// No override field present — leave the request alone.
    None,
    /// Override field present and value is a recognised target method.
    Replace(http::Method),
    /// Override field present but value is not a recognised method.
    Invalid,
}

fn scan_form_for_override(bytes: &[u8], field: &str) -> OverrideOutcome {
    let mut outcome = OverrideOutcome::None;
    for (key, value) in url::form_urlencoded::parse(bytes) {
        if key == field {
            outcome = parse_override_value(&value)
                .map_or(OverrideOutcome::Invalid, OverrideOutcome::Replace);
            break;
        }
    }
    outcome
}

fn is_form_urlencoded(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"))
}

/// Returns `true` only when the request advertises a `Content-Length`
/// strictly larger than `limit`. A missing or unparseable header returns
/// `false`, since we can't make a confident determination without
/// buffering — those cases fall through to the buffered scan.
fn content_length_exceeds(headers: &http::HeaderMap, limit: usize) -> bool {
    headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .is_some_and(|len| len > limit as u64)
}

/// Is the request a same-origin browser form submission?
///
/// The override convention is documented for browser HTML forms, so we
/// only honour it when the request really did originate from the same
/// origin. Without this check a CSRF-disabled (or CSRF-exempt) route
/// declared as `#[delete]` could be reached by a cross-origin form
/// using `_method=DELETE` even though the equivalent direct `DELETE`
/// request would be blocked by the browser's same-origin policy.
///
/// Decision matrix (returns `true` to allow the override):
///
/// 1. `Sec-Fetch-Site` is sent by every browser released since ~2020
///    and is the most reliable signal. We allow `same-origin`,
///    `same-site`, and `none` (user-initiated, no opener) and reject
///    `cross-site`.
/// 2. When `Sec-Fetch-Site` is absent, fall back to comparing the
///    `Origin` header against `Host`. Modern browsers always send
///    `Origin` for `POST`, so a mismatch here means the request really
///    is cross-origin.
/// 3. When both signals are absent, deny the override (fail closed).
///    Such requests are either an extremely old browser or a non-
///    browser client; in either case the documented browser-form
///    convention does not apply, so the safe thing is to forward the
///    request as the original `POST` and let route matching reject it.
fn is_same_origin_form(headers: &http::HeaderMap) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return matches!(site, "same-origin" | "same-site" | "none");
    }

    let origin = headers
        .get(http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let host = headers
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok());

    match (origin, host) {
        (Some(origin), Some(host)) => origin_matches_host(origin, host),
        // No origin and no Sec-Fetch-Site: fail closed.
        _ => false,
    }
}

/// `Origin` is a scheme + host (+ optional port); `Host` is host
/// (+ optional port). We treat them as same-origin when the
/// host:port portion matches.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let origin_authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin);
    // Trim any trailing path (defensive — `Origin` should not include one).
    let origin_authority = origin_authority.split('/').next().unwrap_or("");
    origin_authority.eq_ignore_ascii_case(host)
}

impl<S, ResBody> Service<Request<axum::body::Body>> for MethodOverrideService<S>
where
    S: Service<Request<axum::body::Body>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ResBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<axum::body::Body>) -> Self::Future {
        // Only POST form-urlencoded submissions originating from the
        // same origin are eligible. Same-origin form semantics: nothing
        // else is allowed to silently morph methods, because a request
        // that browsers won't make directly (e.g. a cross-origin
        // `<form>` submitting to a `#[delete]` route) must not gain
        // reachability via the override convention.
        if req.method() != http::Method::POST
            || !is_form_urlencoded(req.headers())
            || !is_same_origin_form(req.headers())
        {
            let mut inner = self.inner.clone();
            std::mem::swap(&mut self.inner, &mut inner);
            return Box::pin(async move { inner.call(req).await });
        }

        // If the client advertises a body larger than the scan cap, skip
        // override processing entirely and forward the original body
        // untouched. We never buffer it, so handlers downstream still
        // receive the full submission verbatim — a form that's too large
        // to scan for `_method` simply doesn't get the override applied.
        if content_length_exceeds(req.headers(), MAX_BODY_SCAN_BYTES) {
            let mut inner = self.inner.clone();
            std::mem::swap(&mut self.inner, &mut inner);
            return Box::pin(async move { inner.call(req).await });
        }

        let config = Arc::clone(&self.config);
        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            // Temporarily take ownership of the body so we can buffer it
            // for parsing without losing the bytes the handler will need.
            let body = std::mem::replace(req.body_mut(), axum::body::Body::empty());
            let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY_SCAN_BYTES).await else {
                // Body exceeded the scan cap while buffering (e.g.
                // chunked transfer with no Content-Length) or read
                // failed. The bytes are gone, so we can't forward a
                // faithful request — but we still want the eventual
                // `413` to flow through the framework's response
                // middleware. Stamp the rejection and pass the request
                // through with an empty body; the inner
                // `method_override_rejection_filter` short-circuits to
                // `413` before any handler is called.
                req.extensions_mut()
                    .insert(MethodOverrideRejection::BodyTooLarge);
                return inner.call(req).await;
            };

            let outcome = scan_form_for_override(&bytes, &config.field_name);

            // Restore the body verbatim regardless of outcome.
            *req.body_mut() = axum::body::Body::from(bytes);

            match outcome {
                OverrideOutcome::None => inner.call(req).await,
                OverrideOutcome::Replace(method) => {
                    let transport = req.method().clone();
                    *req.method_mut() = method.clone();
                    req.extensions_mut().insert(OverriddenMethod {
                        transport,
                        effective: method,
                    });
                    inner.call(req).await
                }
                OverrideOutcome::Invalid => {
                    // Stamp the rejection and forward as POST. The
                    // inner rejection filter converts this to `400`
                    // from inside the framework's response middleware
                    // stack, so security headers, request IDs, metrics,
                    // and error-page rendering all apply.
                    req.extensions_mut()
                        .insert(MethodOverrideRejection::InvalidValue);
                    inner.call(req).await
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::{delete, get, patch, post, put};
    use tower::ServiceExt;

    /// Build a router and wrap it with [`MethodOverrideLayer`] as a top-level
    /// Service.
    ///
    /// `Router::layer` in axum 0.8 applies middleware **per registered
    /// method handler**, which means a layer added that way cannot rewrite
    /// `POST` into `PUT`/`PATCH`/`DELETE` — the inner `MethodRouter`
    /// returns `405` before the layer ever runs. The override convention
    /// requires running before route matching, so wrap the router as a
    /// `tower::Service`.
    ///
    /// The inner `method_override_rejection_filter` is applied via
    /// `Router::layer` so that `MethodOverrideRejection` extensions
    /// stamped by the outer layer are converted into `400`/`413`
    /// responses inside the per-route middleware chain — mirroring the
    /// production wiring in `router::apply_middleware`.
    fn layered_router() -> MethodOverrideService<Router> {
        let router = Router::new()
            .route("/items", post(|| async { "created" }))
            .route("/items/{id}", put(|| async { "put-ok" }))
            .route("/items/{id}", patch(|| async { "patch-ok" }))
            .route("/items/{id}", delete(|| async { "delete-ok" }))
            .route("/items/{id}", get(|| async { "show" }))
            .layer(axum::middleware::from_fn(method_override_rejection_filter));
        MethodOverrideLayer::new().layer(router)
    }

    #[tokio::test]
    async fn post_without_override_field_reaches_post_handler() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("title=hello"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"created");
    }

    #[tokio::test]
    async fn post_with_method_put_reaches_put_handler() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("_method=PUT&title=hi"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"put-ok");
    }

    #[tokio::test]
    async fn post_with_method_patch_reaches_patch_handler() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("_method=patch"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"patch-ok");
    }

    #[tokio::test]
    async fn post_with_method_delete_reaches_delete_handler() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("_method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"delete-ok");
    }

    #[tokio::test]
    async fn invalid_override_value_returns_400() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("_method=BREW"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn override_ignored_without_form_content_type() {
        // JSON POST should NOT have its method rewritten even if a `_method`
        // string happens to appear in the body. The convention is documented
        // for browser HTML form submissions only.
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"_method":"DELETE","title":"hi"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"created");
    }

    #[tokio::test]
    async fn override_ignored_for_get_requests() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/items/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"show");
    }

    #[tokio::test]
    async fn override_preserves_body_for_handler() {
        // Override consumes the body to peek at `_method`; it MUST restore
        // the bytes so handler extractors (Form, ChangesetForm, etc.) work.
        async fn echo(body: String) -> String {
            body
        }
        let router = Router::new().route("/echo", put(echo));
        let app = MethodOverrideLayer::new().layer(router);

        let payload = "_method=PUT&title=hello&count=3";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), payload);
    }

    #[tokio::test]
    async fn override_marks_request_extension() {
        async fn marker(axum::Extension(o): axum::Extension<OverriddenMethod>) -> String {
            format!("{}->{}", o.transport, o.effective)
        }
        let router = Router::new().route("/x", delete(marker));
        let app = MethodOverrideLayer::new().layer(router);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/x")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("_method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"POST->DELETE");
    }

    #[tokio::test]
    async fn custom_field_name_is_respected() {
        let router = Router::new().route("/items/{id}", delete(|| async { "gone" }));
        let app = MethodOverrideLayer::from_config(MethodOverrideConfig {
            field_name: "x-method".into(),
        })
        .layer(router);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("x-method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"gone");
    }

    /// Acceptance criterion: "CSRF protection still treats the effective
    /// overridden method as unsafe, and tests prove an overridden DELETE
    /// without a valid CSRF token is rejected."
    ///
    /// The CSRF layer wraps method-override on the outside. CSRF sees the
    /// browser's `POST` (already an unsafe method) and demands a token.
    /// A `_method=DELETE` body with no `_csrf` field is rejected with
    /// `403 Forbidden`, exactly like any other mutating POST.
    #[tokio::test]
    async fn overridden_delete_without_csrf_token_is_rejected() {
        let csrf_config = crate::security::CsrfConfig {
            enabled: true,
            ..Default::default()
        };
        let router = Router::new()
            .route("/items/{id}", delete(|| async { "deleted" }))
            .layer(crate::security::CsrfLayer::from_config(&csrf_config));
        let app = MethodOverrideLayer::new().layer(router);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .header(http::header::ACCEPT, "text/html")
                    .header("Cookie", "autumn-csrf=valid-cookie-token")
                    .body(Body::from("_method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// Companion: an overridden DELETE with a matching `_csrf` token passes
    /// CSRF and reaches the DELETE handler.
    #[tokio::test]
    async fn overridden_delete_with_csrf_token_reaches_handler() {
        let csrf_config = crate::security::CsrfConfig {
            enabled: true,
            ..Default::default()
        };
        let router = Router::new()
            .route("/items/{id}", delete(|| async { "deleted" }))
            .layer(crate::security::CsrfLayer::from_config(&csrf_config));
        let app = MethodOverrideLayer::new().layer(router);

        let token = "valid-cookie-token";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .header("Cookie", format!("autumn-csrf={token}"))
                    .body(Body::from(format!("_csrf={token}&_method=DELETE")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"deleted");
    }

    /// Regression: a form-urlencoded POST larger than the scan cap must
    /// still reach its handler with the body intact when it doesn't
    /// declare an override. Previously the scan failure path emptied
    /// the body, corrupting any large legitimate POST routed through
    /// the global override layer.
    #[tokio::test]
    async fn large_form_post_without_override_preserves_full_body() {
        async fn measure(body: bytes::Bytes) -> String {
            format!("{}", body.len())
        }
        // Raise the handler-side body limit so the test exercises the
        // middleware's pass-through path rather than axum's own default
        // 2 MiB body cap on extractors.
        let router = Router::new()
            .route("/upload", post(measure))
            .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024));
        let app = MethodOverrideLayer::new().layer(router);

        // 3 MiB payload: comfortably over MAX_BODY_SCAN_BYTES (2 MiB).
        let big = "x".repeat(3 * 1024 * 1024);
        let payload = format!("title={big}");
        let payload_len = payload.len();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/upload")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .header("content-length", payload_len.to_string())
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 32)
            .await
            .unwrap();
        let observed: usize = std::str::from_utf8(&body).unwrap().parse().unwrap();
        assert_eq!(
            observed, payload_len,
            "handler must see the full original body, not an empty one"
        );
    }

    /// A POST whose body actually exceeds the scan cap during buffering
    /// (e.g. chunked transfer encoding without an accurate Content-Length)
    /// is stamped with [`MethodOverrideRejection::BodyTooLarge`] and the
    /// inner `method_override_rejection_filter` converts it into an
    /// explicit `413` — inside the per-route layer chain — rather than
    /// the outer layer short-circuiting and bypassing framework
    /// middleware.
    #[tokio::test]
    async fn unbounded_oversized_form_post_returns_413() {
        async fn handler(body: String) -> String {
            body
        }
        let router = Router::new()
            .route("/x", post(handler))
            .layer(axum::middleware::from_fn(method_override_rejection_filter));
        let app = MethodOverrideLayer::new().layer(router);

        // No Content-Length header -> content_length_exceeds returns
        // false and we attempt to buffer. The body itself is larger
        // than the scan cap so `to_bytes` will error.
        let payload = "x".repeat(3 * 1024 * 1024);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/x")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// When the outer `MethodOverrideLayer` is composed without the
    /// inner `method_override_rejection_filter`, the rejection
    /// extension is still stamped on the request — but no handler
    /// short-circuits, so the request continues to flow normally.
    /// This matches how the framework wires the two pieces, and
    /// proves that the outer layer doesn't generate `400`/`413`
    /// responses on its own.
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static OBSERVED_REJECTION: OnceLock<Mutex<Option<MethodOverrideRejection>>> = OnceLock::new();

    async fn capture_rejection(
        request: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        if let Some(rej) = request
            .extensions()
            .get::<MethodOverrideRejection>()
            .copied()
        {
            *OBSERVED_REJECTION
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap() = Some(rej);
        }
        next.run(request).await
    }

    #[tokio::test]
    async fn outer_layer_stamps_extension_without_short_circuiting() {
        let cell = OBSERVED_REJECTION.get_or_init(|| Mutex::new(None));
        cell.lock().unwrap().take();

        let router = Router::new()
            .route("/x", post(|| async { "post-ok" }))
            .layer(axum::middleware::from_fn(capture_rejection));
        let app = MethodOverrideLayer::new().layer(router);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/x")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("_method=BREW"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            *cell.lock().unwrap(),
            Some(MethodOverrideRejection::InvalidValue)
        );
    }

    /// A POST carrying `_method=DELETE` from a cross-site context must
    /// NOT be honoured as a DELETE. Without this check, a route declared
    /// as `#[delete]` only — which native browser forms can never reach
    /// directly — would become reachable from any third-party site that
    /// renders a form pointing at it (when CSRF is disabled or the path
    /// is CSRF-exempt).
    #[tokio::test]
    async fn cross_site_form_is_not_honoured_as_override() {
        // Inner /items/{id} only has DELETE; no POST handler. If the
        // override were applied, we'd see 200 "delete-ok". If the
        // override is correctly skipped, we should see 405 (POST not
        // allowed for that route).
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::from("_method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "cross-site override must not be applied; expected the inner \
             router to reject the underlying POST"
        );
    }

    /// When `Sec-Fetch-Site` is absent we fall back to comparing
    /// `Origin` with `Host`. A mismatch means the request is
    /// cross-origin and the override must not apply.
    #[tokio::test]
    async fn origin_host_mismatch_is_not_honoured_as_override() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("origin", "https://evil.example")
                    .header("host", "app.example")
                    .body(Body::from("_method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// `Origin` and `Host` match (both `app.example`) — same origin via
    /// the fallback path; the override should apply.
    #[tokio::test]
    async fn origin_host_match_is_honoured_as_override() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("origin", "https://app.example")
                    .header("host", "app.example")
                    .body(Body::from("_method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Neither `Sec-Fetch-Site` nor `Origin` present: fail closed. The
    /// override is not applied — the request flows through as POST.
    #[tokio::test]
    async fn missing_origin_signals_fail_closed() {
        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("_method=DELETE"))
                    .unwrap(),
            )
            .await
            .unwrap();

        // /items/1 has no POST handler, so a non-overridden POST yields 405.
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// Helper-level cases that don't require the full router wiring.
    #[test]
    fn is_same_origin_form_decision_matrix() {
        // Sec-Fetch-Site values
        for site in ["same-origin", "same-site", "none"] {
            let mut h = http::HeaderMap::new();
            h.insert("sec-fetch-site", http::HeaderValue::from_str(site).unwrap());
            assert!(
                is_same_origin_form(&h),
                "sec-fetch-site={site} should be allowed"
            );
        }
        let mut h = http::HeaderMap::new();
        h.insert(
            "sec-fetch-site",
            http::HeaderValue::from_static("cross-site"),
        );
        assert!(!is_same_origin_form(&h));

        // Origin matches Host
        let mut h = http::HeaderMap::new();
        h.insert(
            http::header::ORIGIN,
            http::HeaderValue::from_static("https://app.example"),
        );
        h.insert(
            http::header::HOST,
            http::HeaderValue::from_static("app.example"),
        );
        assert!(is_same_origin_form(&h));

        // Origin does not match Host
        let mut h = http::HeaderMap::new();
        h.insert(
            http::header::ORIGIN,
            http::HeaderValue::from_static("https://evil.example"),
        );
        h.insert(
            http::header::HOST,
            http::HeaderValue::from_static("app.example"),
        );
        assert!(!is_same_origin_form(&h));

        // Neither signal present: fail closed.
        let h = http::HeaderMap::new();
        assert!(!is_same_origin_form(&h));
    }

    /// Override rejection responses carry an `AutumnErrorInfo` extension
    /// so the framework exception filter chain — problem-details
    /// negotiation, custom HTML error-page rendering — processes them
    /// the same as a handler-generated error response.
    #[tokio::test]
    async fn rejection_response_carries_autumn_error_info() {
        use crate::middleware::exception_filter::AutumnErrorInfo;

        let app = layered_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/items/1")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from("_method=BREW"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let info = response
            .extensions()
            .get::<AutumnErrorInfo>()
            .expect("override rejection must carry AutumnErrorInfo");
        assert_eq!(info.status, StatusCode::BAD_REQUEST);
        assert!(info.message.contains("PUT, PATCH, or DELETE"));
    }

    #[test]
    fn content_length_exceeds_only_when_strictly_larger() {
        let mut headers = http::HeaderMap::new();
        assert!(!content_length_exceeds(&headers, 1024));

        headers.insert(
            http::header::CONTENT_LENGTH,
            http::HeaderValue::from_static("1024"),
        );
        assert!(!content_length_exceeds(&headers, 1024));

        headers.insert(
            http::header::CONTENT_LENGTH,
            http::HeaderValue::from_static("1025"),
        );
        assert!(content_length_exceeds(&headers, 1024));

        // Unparseable header values stay conservative: don't short-circuit.
        headers.insert(
            http::header::CONTENT_LENGTH,
            http::HeaderValue::from_static("not-a-number"),
        );
        assert!(!content_length_exceeds(&headers, 1024));
    }
}
