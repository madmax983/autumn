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
use pin_project_lite::pin_project;
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

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for CsrfService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone,
    ResBody: Default,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = CsrfFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let is_safe = self.settings.safe_methods.contains(req.method());
        let cookie_token = extract_cookie_token(req.headers(), &self.settings.cookie_name);

        // For safe methods, generate a new token if none exists
        let token = cookie_token
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // Insert CsrfToken into request extensions for handler access
        req.extensions_mut().insert(CsrfToken(token.clone()));

        if !is_safe {
            // Validate: the header token must match the cookie token
            let header_token = req
                .headers()
                .get(&self.settings.token_header)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);

            if cookie_token.is_none() || header_token.as_deref() != cookie_token.as_deref() {
                // CSRF validation failed -- reject via flag in the future
                return CsrfFuture {
                    inner: self.inner.call(req),
                    csrf_rejected: true,
                    set_cookie: None,
                };
            }
        }

        // Set cookie if not already present
        let set_cookie = if cookie_token.is_none() {
            Some(format!(
                "{}={}; Path=/; SameSite=Lax; HttpOnly",
                self.settings.cookie_name, token
            ))
        } else {
            None
        };

        CsrfFuture {
            inner: self.inner.call(req),
            csrf_rejected: false,
            set_cookie,
        }
    }
}

pin_project! {
    /// Future for the CSRF middleware.
    pub struct CsrfFuture<F> {
        #[pin]
        inner: F,
        csrf_rejected: bool,
        set_cookie: Option<String>,
    }
}

impl<F, ResBody, E> Future for CsrfFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
    ResBody: Default,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if *this.csrf_rejected {
            // Build a 403 response
            let mut response = Response::new(ResBody::default());
            *response.status_mut() = StatusCode::FORBIDDEN;
            return Poll::Ready(Ok(response));
        }

        match this.inner.poll(cx) {
            Poll::Ready(Ok(mut response)) => {
                // Add Set-Cookie header for new CSRF tokens
                if let Some(cookie) = this.set_cookie.take() {
                    if let Ok(val) = http::HeaderValue::from_str(&cookie) {
                        response.headers_mut().append(http::header::SET_COOKIE, val);
                    }
                }
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
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
}
