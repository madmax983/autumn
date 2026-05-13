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
//! # Failure modes
//!
//! - An unrecognised override value (e.g. `_method=BREW`) is rejected
//!   with `400 Bad Request` before reaching the handler.
//! - The override only acts on `POST` requests with a form-style body.
//!   Headers like `X-HTTP-Method-Override` are intentionally **not**
//!   honoured: this convention is documented for browser HTML forms
//!   only, not for arbitrary REST tunneling.
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

impl<S, ResBody> Service<Request<axum::body::Body>> for MethodOverrideService<S>
where
    S: Service<Request<axum::body::Body>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ResBody: From<&'static str> + Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<axum::body::Body>) -> Self::Future {
        // Only POST requests are eligible for override. Same-origin form
        // semantics: nothing else is allowed to silently morph methods.
        if req.method() != http::Method::POST || !is_form_urlencoded(req.headers()) {
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
            let bytes = axum::body::to_bytes(body, MAX_BODY_SCAN_BYTES)
                .await
                .unwrap_or_else(|_| axum::body::Bytes::new());

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
                    let mut response = Response::new(ResBody::from(
                        "Invalid method override value: must be PUT, PATCH, or DELETE.",
                    ));
                    *response.status_mut() = StatusCode::BAD_REQUEST;
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        http::HeaderValue::from_static("text/plain; charset=utf-8"),
                    );
                    Ok(response)
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
    fn layered_router() -> MethodOverrideService<Router> {
        let router = Router::new()
            .route("/items", post(|| async { "created" }))
            .route("/items/{id}", put(|| async { "put-ok" }))
            .route("/items/{id}", patch(|| async { "patch-ok" }))
            .route("/items/{id}", delete(|| async { "delete-ok" }))
            .route("/items/{id}", get(|| async { "show" }));
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
}
