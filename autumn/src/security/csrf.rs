//! CSRF (Cross-Site Request Forgery) protection middleware.
//!
//! Protects against CSRF attacks by requiring a token on mutating
//! HTTP methods (POST, PUT, DELETE, PATCH). The token is stored in a
//! cookie and must be echoed back via a request header or form field.
//!
//! # How it works
//!
//! 1. On every response, a CSRF cookie is set (if not already present)
//!    containing a random UUID v4 token.
//! 2. On mutating requests, the middleware checks that the token from
//!    the cookie matches the token in the `X-CSRF-Token` header (or
//!    `_csrf` form field).
//! 3. Safe methods (GET, HEAD, OPTIONS, TRACE) are exempt.
//!
//! # Configuration
//!
//! See [`CsrfConfig`] for available settings.
//!
//! # Examples
//!
//! ## Template integration (Maud)
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::security::CsrfToken;
//!
//! #[get("/form")]
//! async fn form(csrf: CsrfToken) -> Markup {
//!     html! {
//!         form method="POST" action="/submit" {
//!             input type="hidden" name="_csrf" value=(csrf.token());
//!             input type="text" name="title";
//!             button { "Submit" }
//!         }
//!     }
//! }
//! ```
//!
//! ## JavaScript / htmx
//!
//! Read the CSRF token from the `autumn-csrf` cookie and send it
//! as an `X-CSRF-Token` header with every mutating request.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::FromRequestParts;
use axum::http::{Request, Response, StatusCode};
use http::header::HeaderName;

use tower::{Layer, Service};
use uuid::Uuid;

use super::config::CsrfConfig;

/// A CSRF token extracted from the request.
///
/// Use this as a handler parameter to access the CSRF token for embedding
/// in HTML forms. The token is generated per-request and stored in
/// request extensions by the [`CsrfLayer`].
///
/// # Examples
///
/// ```rust,ignore
/// use autumn_web::prelude::*;
/// use autumn_web::security::CsrfToken;
///
/// #[get("/edit")]
/// async fn edit_form(csrf: CsrfToken) -> Markup {
///     html! {
///         form method="POST" {
///             input type="hidden" name="_csrf" value=(csrf.token());
///             // ...
///         }
///     }
/// }
/// ```
#[derive(Clone, Debug)]
pub struct CsrfToken(String);

impl CsrfToken {
    /// Returns the CSRF token value for embedding in forms or headers.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CsrfToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<S> FromRequestParts<S> for CsrfToken
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Self>().cloned().ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "CSRF token not found in request extensions. Is CsrfLayer enabled?",
        ))
    }
}

/// Shared CSRF configuration.
#[derive(Debug, Clone)]
struct CsrfSettings {
    cookie_name: String,
    token_header: HeaderName,
    form_field: String,
    safe_methods: Vec<http::Method>,
}

/// Tower [`Layer`] that applies CSRF protection.
///
/// Applied automatically when `security.csrf.enabled = true` in config.
#[derive(Clone, Debug)]
pub struct CsrfLayer {
    settings: Arc<CsrfSettings>,
}

impl CsrfLayer {
    /// Create a new CSRF layer from configuration.
    #[must_use]
    pub fn from_config(config: &CsrfConfig) -> Self {
        let safe_methods = config
            .safe_methods
            .iter()
            .filter_map(|m| m.parse::<http::Method>().ok())
            .collect();

        let token_header = config
            .token_header
            .parse::<HeaderName>()
            .unwrap_or_else(|_| HeaderName::from_static("x-csrf-token"));

        Self {
            settings: Arc::new(CsrfSettings {
                cookie_name: config.cookie_name.clone(),
                token_header,
                form_field: config.form_field.clone(),
                safe_methods,
            }),
        }
    }
}

impl<S> Layer<S> for CsrfLayer {
    type Service = CsrfService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CsrfService {
            inner,
            settings: Arc::clone(&self.settings),
        }
    }
}

/// Tower [`Service`] produced by [`CsrfLayer`].
#[derive(Clone, Debug)]
pub struct CsrfService<S> {
    inner: S,
    settings: Arc<CsrfSettings>,
}

/// Constant-time string comparison to prevent timing attacks when verifying CSRF tokens.
#[inline(never)]
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        // Prevent compiler from optimizing out the bitwise operations
        result |= x ^ y;
    }
    // ensure result is evaluated using std::hint::black_box to defeat compiler optimizations
    std::hint::black_box(result) == 0
}

/// Extract the CSRF cookie value from the Cookie header.
fn extract_cookie_token(req_headers: &http::HeaderMap, cookie_name: &str) -> Option<String> {
    req_headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|cookie_str| cookie_str.split(';'))
        .map(str::trim)
        .find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            if name.trim() == cookie_name {
                Some(value.trim().to_owned())
            } else {
                None
            }
        })
}

impl<S, ResBody> Service<Request<axum::body::Body>> for CsrfService<S>
where
    S: Service<Request<axum::body::Body>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<axum::body::Body>) -> Self::Future {
        let is_safe = self.settings.safe_methods.contains(req.method());
        let cookie_token = extract_cookie_token(req.headers(), &self.settings.cookie_name);

        // For safe methods, generate a new token if none exists
        let token = cookie_token
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // Insert CsrfToken into request extensions for handler access
        req.extensions_mut().insert(CsrfToken(token.clone()));

        // Check if we need to set a cookie
        let set_cookie = if cookie_token.is_none() {
            Some(format!(
                "{}={}; Path=/; SameSite=Lax; HttpOnly",
                self.settings.cookie_name, token
            ))
        } else {
            None
        };

        let settings = Arc::clone(&self.settings);
        let mut inner = self.inner.clone();

        // Swap to ensure correct poll_ready semantics
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            if !is_safe {
                // 1. Check header
                let mut token_found = false;

                let header_token = req
                    .headers()
                    .get(&settings.token_header)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);

                if let (Some(c), Some(h)) = (&cookie_token, &header_token) {
                    if !c.is_empty() && !h.is_empty() && constant_time_eq(c, h) {
                        token_found = true;
                    }
                }

                // 2. Check form field (if not found in header)
                if !token_found {
                    let content_type = req
                        .headers()
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default();

                    if content_type.starts_with("application/x-www-form-urlencoded") {
                        let (parts, body) = req.into_parts();
                        // Limit body size to avoid DoS when extracting form field
                        let bytes = axum::body::to_bytes(body, 2 * 1024 * 1024)
                            .await
                            .unwrap_or_else(|_| axum::body::Bytes::new());

                        if let Ok(body_str) = std::str::from_utf8(&bytes) {
                            for pair in body_str.split('&') {
                                if let Some((key, value)) = pair.split_once('=') {
                                    if key == settings.form_field {
                                        // Simple URL decoding by replacing + with space and % encoded chars
                                        // Note: CSRF tokens are UUIDs, so they shouldn't contain special chars anyway
                                        if let Some(c) = &cookie_token {
                                            if !c.is_empty()
                                                && !value.is_empty()
                                                && constant_time_eq(c, value)
                                            {
                                                token_found = true;
                                            }
                                        }
                                        break;
                                    }
                                }
                            }
                        }

                        // Reconstruct request
                        req = Request::from_parts(parts, axum::body::Body::from(bytes));
                    }
                }

                if !token_found {
                    // Validation failed, reject immediately
                    let mut response = Response::new(ResBody::default());
                    *response.status_mut() = StatusCode::FORBIDDEN;
                    return Ok(response);
                }
            }

            // Validation passed (or method is safe)
            let mut response = inner.call(req).await?;

            if let Some(cookie) = set_cookie {
                if let Ok(val) = http::header::HeaderValue::from_str(&cookie) {
                    response.headers_mut().append(http::header::SET_COOKIE, val);
                }
            }

            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn default_csrf_config() -> CsrfConfig {
        CsrfConfig {
            enabled: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn safe_method_passes_without_token() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn safe_method_sets_csrf_cookie() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let set_cookie = response
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.starts_with("autumn-csrf="));
        assert!(set_cookie.contains("HttpOnly"));
    }

    #[tokio::test]
    async fn post_without_token_returns_403() {
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_with_valid_token_passes() {
        let token = Uuid::new_v4().to_string();
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", format!("autumn-csrf={token}"))
                    .header("X-CSRF-Token", &token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_with_mismatched_token_returns_403() {
        let cookie_token = Uuid::new_v4().to_string();
        let header_token = Uuid::new_v4().to_string();
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", format!("autumn-csrf={cookie_token}"))
                    .header("X-CSRF-Token", &header_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn csrf_token_extractor_works() {
        async fn handler(csrf: CsrfToken) -> String {
            csrf.token().to_owned()
        }

        let app = Router::new()
            .route("/", get(handler))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let token_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(Uuid::parse_str(&token_str).is_ok());
    }

    #[test]
    fn extract_cookie_from_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            "autumn-csrf=abc123; other=xyz".parse().unwrap(),
        );
        assert_eq!(
            extract_cookie_token(&headers, "autumn-csrf"),
            Some("abc123".to_owned())
        );
    }

    #[test]
    fn missing_cookie_returns_none() {
        let headers = http::HeaderMap::new();
        assert_eq!(extract_cookie_token(&headers, "autumn-csrf"), None);
    }

    #[test]
    fn extract_cookie_ignores_malformed_cookies() {
        let mut headers = http::HeaderMap::new();
        // Missing '='
        headers.insert(http::header::COOKIE, "autumn-csrf abc123".parse().unwrap());
        assert_eq!(extract_cookie_token(&headers, "autumn-csrf"), None);

        // Multiple spaces
        headers.insert(
            http::header::COOKIE,
            "   autumn-csrf  =  abc123  ; other=xyz".parse().unwrap(),
        );
        assert_eq!(
            extract_cookie_token(&headers, "autumn-csrf"),
            Some("abc123".to_owned())
        );
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(super::constant_time_eq("abc", "abc"));
        assert!(!super::constant_time_eq("abc", "ab"));
        assert!(!super::constant_time_eq("abc", "abd"));
        assert!(super::constant_time_eq("", ""));
        assert!(!super::constant_time_eq("a", "b"));
        assert!(!super::constant_time_eq("a", "A"));
    }

    #[tokio::test]
    async fn post_with_empty_cookie_but_valid_header() {
        let token = Uuid::new_v4().to_string();
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", "autumn-csrf=")
                    .header("X-CSRF-Token", &token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_with_valid_cookie_but_empty_header() {
        let token = Uuid::new_v4().to_string();
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", format!("autumn-csrf={token}"))
                    .header("X-CSRF-Token", "")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_with_empty_cookie_but_valid_form_field() {
        let token = Uuid::new_v4().to_string();
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", "autumn-csrf=")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("_csrf={token}")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_with_valid_cookie_but_empty_form_field() {
        let token = Uuid::new_v4().to_string();
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", format!("autumn-csrf={token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("_csrf="))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_with_large_body_fails_csrf() {
        let token = Uuid::new_v4().to_string();
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        // Create a body just slightly over 2MB. The CSRF extractor limits to 2MB.
        let large_padding = "a".repeat(2 * 1024 * 1024 + 10);
        let body_content = format!("_csrf={token}&pad={large_padding}");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", format!("autumn-csrf={token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(body_content))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn post_with_empty_tokens_returns_403() {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use axum::routing::post;
    use tower::ServiceExt;

    let app = Router::new()
        .route("/submit", post(|| async { "created" }))
        .layer(CsrfLayer::from_config(&CsrfConfig {
            enabled: true,
            ..Default::default()
        }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/submit")
                .header("Cookie", "autumn-csrf=")
                .header("X-CSRF-Token", "")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_with_empty_form_tokens_returns_403() {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use axum::routing::post;
    use tower::ServiceExt;

    let app = Router::new()
        .route("/submit", post(|| async { "created" }))
        .layer(CsrfLayer::from_config(&CsrfConfig {
            enabled: true,
            ..Default::default()
        }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/submit")
                .header("Cookie", "autumn-csrf=")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf="))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
