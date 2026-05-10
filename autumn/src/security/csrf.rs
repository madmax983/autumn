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

use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use axum::http::{Request, Response, StatusCode};
use http::header::HeaderName;

use tower::{Layer, Service};
use uuid::Uuid;

use super::config::CsrfConfig;

/// Error body returned with a `403 Forbidden` when CSRF validation fails.
const CSRF_FORBIDDEN_MESSAGE: &str = "CSRF token missing or invalid";

/// The configured CSRF form field name, placed in request extensions by [`CsrfLayer`].
///
/// [`ChangesetForm`](crate::form::ChangesetForm) reads this so `form_tag` emits the
/// hidden input under the correct field name even when `security.csrf.form_field` has
/// been customised from its default `"_csrf"`.
#[derive(Clone, Debug)]
pub struct CsrfFormField(pub String);

/// A CSRF token extracted from the request.
///
/// Use this as a handler parameter to access the CSRF token for embedding
/// in HTML forms. The token is generated per-request and stored in
/// request extensions by the [`CsrfLayer`].
///
/// ## Examples
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

    #[cfg(test)]
    pub(crate) const fn new(token: String) -> Self {
        Self(token)
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

impl<S> OptionalFromRequestParts<S> for CsrfToken
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<Self>().cloned())
    }
}

impl<S> FromRequestParts<S> for CsrfFormField
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
            "CSRF form field not found in request extensions. Is CsrfLayer enabled?",
        ))
    }
}

impl<S> OptionalFromRequestParts<S> for CsrfFormField
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<Self>().cloned())
    }
}

/// Shared CSRF configuration.
#[derive(Debug, Clone)]
struct CsrfSettings {
    cookie_name: String,
    token_header: HeaderName,
    form_field: String,
    safe_methods: Vec<http::Method>,
    exempt_paths: Vec<String>,
    signing_keys: Option<Arc<crate::security::config::ResolvedSigningKeys>>,
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
                exempt_paths: config.exempt_paths.clone(),
                signing_keys: None,
            }),
        }
    }

    /// Attach signing keys so CSRF tokens are HMAC-signed.
    ///
    /// When set, tokens are in `{uuid}.{hmac_hex}` format. Unsigned tokens are
    /// rejected. `previous` keys in [`ResolvedSigningKeys`] allow tokens signed
    /// with an old key to remain valid during a rotation grace window.
    #[must_use]
    pub fn with_signing_keys(
        mut self,
        keys: Arc<crate::security::config::ResolvedSigningKeys>,
    ) -> Self {
        Arc::make_mut(&mut self.settings).signing_keys = Some(keys);
        self
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

use subtle::{Choice, ConstantTimeEq};

/// Constant-time string comparison to prevent timing attacks when verifying CSRF tokens.
///
/// The comparison always processes exactly `b.len()` bytes so that execution
/// time is independent of the length of the submitted token `a`.  Neither a
/// length mismatch nor a short input causes an early exit.
#[inline(never)]
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();

    // Constant-time length check — no early exit.
    let len_eq = a.len().ct_eq(&b.len());

    // Iterate over `a` (the trusted stored token) so the loop count is fixed
    // at the server-side token length, regardless of what the caller submits
    // as `b`.  Callers pass the attacker-controlled value as `b`, so iterating
    // over `a` ensures every submission — short or long — executes the same
    // amount of work.  Out-of-range positions in `b` use the sentinel 0xFF,
    // which can never match a valid ASCII/UTF-8 token byte.
    let mut bytes_eq = Choice::from(1u8);
    for (i, &a_byte) in a.iter().enumerate() {
        let b_byte = *b.get(i).unwrap_or(&0xFF);
        bytes_eq &= a_byte.ct_eq(&b_byte);
    }

    (len_eq & bytes_eq).into()
}

/// Extract the CSRF cookie value from the Cookie header.
fn extract_cookie_token(req_headers: &http::HeaderMap, cookie_name: &str) -> Option<String> {
    let mut found_token = None;

    for cookie_header in &req_headers.get_all(http::header::COOKIE) {
        let Ok(cookie_str) = cookie_header.to_str() else {
            continue;
        };

        for pair in cookie_str.split(';') {
            let pair = pair.trim();
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };

            if name.trim() != cookie_name {
                continue;
            }

            if found_token.is_some() {
                // Multiple cookies with the same name found.
                // This indicates a potential Cookie Tossing attack!
                // Reject by returning None.
                return None;
            }

            found_token = Some(value.trim().to_owned());
        }
    }

    found_token
}

impl<S, ResBody> Service<Request<axum::body::Body>> for CsrfService<S>
where
    S: Service<Request<axum::body::Body>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ResBody: From<&'static str> + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<axum::body::Body>) -> Self::Future {
        let path = req.uri().path();
        let is_exempt = self
            .settings
            .exempt_paths
            .iter()
            .any(|prefix| path.starts_with(prefix.as_str()));
        let is_safe = is_exempt || self.settings.safe_methods.contains(req.method());
        let cookie_token = extract_cookie_token(req.headers(), &self.settings.cookie_name);

        // Generate a new token if none exists in the cookie.
        // When signing keys are active, the token is {uuid}.{hmac_hex}.
        let token = cookie_token.clone().unwrap_or_else(|| {
            let raw = Uuid::new_v4().to_string();
            if let Some(keys) = &self.settings.signing_keys {
                let sig = keys.sign(raw.as_bytes());
                format!("{raw}.{sig}")
            } else {
                raw
            }
        });

        // Insert CsrfToken and the configured form field name into request extensions.
        req.extensions_mut().insert(CsrfToken(token.clone()));
        req.extensions_mut()
            .insert(CsrfFormField(self.settings.form_field.clone()));

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
            if !is_safe && !verify_csrf_token(&mut req, &settings, cookie_token.as_deref()).await {
                let mut response = Response::new(ResBody::from(CSRF_FORBIDDEN_MESSAGE));
                *response.status_mut() = StatusCode::FORBIDDEN;
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static("text/plain; charset=utf-8"),
                );
                return Ok(response);
            }

            // Validation passed (or method is safe)
            let mut response = inner.call(req).await?;

            if let Some(cookie) = set_cookie
                && let Ok(val) = http::header::HeaderValue::from_str(&cookie)
            {
                response.headers_mut().append(http::header::SET_COOKIE, val);
            }

            Ok(response)
        })
    }
}

/// Validate a CSRF cookie token's HMAC when signing is active.
///
/// Returns `false` when signing keys are set but the token is unsigned or carries
/// an invalid HMAC (catches tampered or pre-rotation unsigned tokens).
fn validate_cookie_token_hmac(cookie_token: &str, settings: &CsrfSettings) -> bool {
    let Some(keys) = &settings.signing_keys else {
        return true; // signing not active — accept raw token
    };
    // Signed format: "{uuid}.{hmac_hex}"
    let Some((uuid_part, sig)) = cookie_token.split_once('.') else {
        return false; // unsigned token rejected when signing is required
    };
    keys.verify(uuid_part.as_bytes(), sig)
}

async fn verify_csrf_token(
    req: &mut Request<axum::body::Body>,
    settings: &CsrfSettings,
    cookie_token: Option<&str>,
) -> bool {
    let mut token_found = false;

    // 1. Check header
    let header_token = req
        .headers()
        .get(&settings.token_header)
        .and_then(|v| v.to_str().ok());

    if let (Some(c), Some(h)) = (cookie_token, header_token)
        && !c.is_empty()
        && !h.is_empty()
        && validate_cookie_token_hmac(c, settings)
        && constant_time_eq(c, h)
    {
        token_found = true;
    }

    if token_found {
        return true;
    }

    // 2. Check form field (if not found in header)
    let content_type = req
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    if !content_type.starts_with("application/x-www-form-urlencoded") {
        return false;
    }

    // Temporarily take ownership of the body
    let body = std::mem::replace(req.body_mut(), axum::body::Body::empty());

    // Limit body size to avoid DoS when extracting form field
    let bytes = axum::body::to_bytes(body, 2 * 1024 * 1024)
        .await
        .unwrap_or_else(|_| axum::body::Bytes::new());

    for (key, value) in url::form_urlencoded::parse(&bytes) {
        if key == settings.form_field {
            if let Some(c) = cookie_token
                && !c.is_empty()
                && !value.is_empty()
                && validate_cookie_token_hmac(c, settings)
                && constant_time_eq(c, value.as_ref())
            {
                token_found = true;
            }
            break;
        }
    }

    // Restore request body
    *req.body_mut() = axum::body::Body::from(bytes);

    token_found
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn post_with_url_encoded_token_passes() {
        let raw_token = "abc+123/xyz=456";
        let encoded_token = "abc%2B123%2Fxyz%3D456";
        let app = Router::new()
            .route("/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&default_csrf_config()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .header("Cookie", format!("autumn-csrf={raw_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("_csrf={encoded_token}")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

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
    async fn forbidden_response_has_clear_error_body() {
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
        assert_eq!(
            response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or_default()),
            Some("text/plain; charset=utf-8")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        assert!(
            text.contains("CSRF"),
            "expected CSRF error message, got: {text:?}"
        );
    }

    #[tokio::test]
    async fn exempt_path_skips_csrf_validation() {
        let config = CsrfConfig {
            enabled: true,
            exempt_paths: vec!["/api/".to_string()],
            ..Default::default()
        };
        let app = Router::new()
            .route("/api/items", post(|| async { "created" }))
            .route("/form/submit", post(|| async { "created" }))
            .layer(CsrfLayer::from_config(&config));

        // Exempt API path: POST with no token should succeed.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/items")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Non-exempt form path: POST with no token should still be blocked.
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/submit")
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
    fn extract_cookie_rejects_multiple_cookies() {
        // Multiple cookies with the same name in a single header
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            "autumn-csrf=abc123; autumn-csrf=xyz456".parse().unwrap(),
        );
        assert_eq!(extract_cookie_token(&headers, "autumn-csrf"), None);

        // Multiple headers with the same cookie
        let mut headers2 = http::HeaderMap::new();
        headers2.append(http::header::COOKIE, "autumn-csrf=abc123".parse().unwrap());
        headers2.append(http::header::COOKIE, "autumn-csrf=xyz456".parse().unwrap());
        assert_eq!(extract_cookie_token(&headers2, "autumn-csrf"), None);
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

    #[tokio::test]
    async fn post_with_empty_tokens_returns_403() {
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

    #[test]
    fn from_config_filters_invalid_methods() {
        let config = CsrfConfig {
            safe_methods: vec![
                "GET".to_string(),
                "INVALID METHOD".to_string(),
                "POST".to_string(),
            ],
            ..Default::default()
        };
        let layer = CsrfLayer::from_config(&config);
        assert_eq!(layer.settings.safe_methods.len(), 2);
        assert!(layer.settings.safe_methods.contains(&http::Method::GET));
        assert!(layer.settings.safe_methods.contains(&http::Method::POST));
    }

    #[test]
    fn from_config_handles_invalid_header_name() {
        let config = CsrfConfig {
            token_header: "Invalid Header Name\n".to_string(),
            ..Default::default()
        };
        let layer = CsrfLayer::from_config(&config);
        assert_eq!(layer.settings.token_header.as_str(), "x-csrf-token");
    }

    // ── Signed CSRF tokens (RED phase) ────────────────────────────────────

    #[tokio::test]
    async fn csrf_token_is_hmac_signed_when_keys_set() {
        use crate::security::config::{SigningSecretConfig, resolve_signing_keys};
        use std::sync::Arc;

        let keys = Arc::new(resolve_signing_keys(&SigningSecretConfig {
            secret: Some("k".repeat(32)),
            previous_secrets: vec![],
        }));
        let layer = CsrfLayer::from_config(&default_csrf_config()).with_signing_keys(keys);

        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(layer);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .expect("should set CSRF cookie")
            .to_str()
            .unwrap();
        let cookie_value = set_cookie
            .split('=')
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .trim();

        assert!(
            cookie_value.contains('.'),
            "signed CSRF cookie must be {{uuid}}.{{hmac}}, got: {cookie_value}"
        );
        let (_uuid_part, sig_part) = cookie_value.split_once('.').unwrap();
        assert_eq!(sig_part.len(), 64, "HMAC hex must be 64 chars");
    }

    #[tokio::test]
    async fn csrf_signed_token_validates_on_post() {
        use crate::security::config::{SigningSecretConfig, resolve_signing_keys};
        use std::sync::Arc;

        let keys = Arc::new(resolve_signing_keys(&SigningSecretConfig {
            secret: Some("k".repeat(32)),
            previous_secrets: vec![],
        }));
        let layer = CsrfLayer::from_config(&default_csrf_config()).with_signing_keys(keys);

        let app = Router::new()
            .route("/", post(|| async { "created" }))
            .layer(layer);

        // Mint a valid signed token
        let config = SigningSecretConfig {
            secret: Some("k".repeat(32)),
            previous_secrets: vec![],
        };
        let signing_keys = resolve_signing_keys(&config);
        let uuid = uuid::Uuid::new_v4().to_string();
        let sig = signing_keys.sign(uuid.as_bytes());
        let signed_token = format!("{uuid}.{sig}");

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("Cookie", format!("autumn-csrf={signed_token}"))
                    .header("X-CSRF-Token", &signed_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_unsigned_token_rejected_when_signing_active() {
        use crate::security::config::{SigningSecretConfig, resolve_signing_keys};
        use std::sync::Arc;

        let keys = Arc::new(resolve_signing_keys(&SigningSecretConfig {
            secret: Some("k".repeat(32)),
            previous_secrets: vec![],
        }));
        let layer = CsrfLayer::from_config(&default_csrf_config()).with_signing_keys(keys);

        let app = Router::new()
            .route("/", post(|| async { "created" }))
            .layer(layer);

        // Raw UUID without HMAC — should be rejected when signing is active
        let raw_token = uuid::Uuid::new_v4().to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("Cookie", format!("autumn-csrf={raw_token}"))
                    .header("X-CSRF-Token", &raw_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "unsigned CSRF token must be rejected when signing is active"
        );
    }

    #[tokio::test]
    async fn csrf_previous_key_signed_token_accepted() {
        use crate::security::config::{ResolvedSigningKeys, SigningSecretConfig, resolve_signing_keys};
        use std::sync::Arc;

        let old_secret = "old-key".repeat(5); // 35 bytes
        let old_keys = resolve_signing_keys(&SigningSecretConfig {
            secret: Some(old_secret.clone()),
            previous_secrets: vec![],
        });

        let uuid = uuid::Uuid::new_v4().to_string();
        let old_sig = old_keys.sign(uuid.as_bytes());
        let old_signed_token = format!("{uuid}.{old_sig}");

        let new_keys = Arc::new(ResolvedSigningKeys::new(
            "new-key".repeat(5).into_bytes(),
            vec![old_secret.into_bytes()],
        ));
        let layer = CsrfLayer::from_config(&default_csrf_config()).with_signing_keys(new_keys);

        let app = Router::new()
            .route("/", post(|| async { "created" }))
            .layer(layer);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("Cookie", format!("autumn-csrf={old_signed_token}"))
                    .header("X-CSRF-Token", &old_signed_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "previous-key-signed CSRF token must pass during grace window"
        );
    }
}
