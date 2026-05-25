//! Security response headers middleware.
//!
//! Applies OWASP-recommended security headers to every HTTP response.
//! The headers are configured via [`HeadersConfig`]
//! in `autumn.toml` under `[security.headers]`.
//!
//! # Headers applied
//!
//! | Header | When | Default |
//! |--------|------|---------|
//! | `X-Frame-Options` | `x_frame_options` is non-empty | `DENY` |
//! | `X-Content-Type-Options` | `x_content_type_options` is `true` | `nosniff` |
//! | `X-XSS-Protection` | `xss_protection` is `true` | `1; mode=block` |
//! | `Strict-Transport-Security` | `strict_transport_security` is `true` | `max-age=31536000; includeSubDomains` |
//! | `Content-Security-Policy` | `content_security_policy` is non-empty | htmx-compatible same-origin policy |
//! | `Referrer-Policy` | `referrer_policy` is non-empty | `strict-origin-when-cross-origin` |
//! | `Permissions-Policy` | `permissions_policy` is non-empty | not sent |
//!
//! # Examples
//!
//! The layer is applied automatically by [`AppBuilder::run`](crate::app::AppBuilder::run).
//! For custom Axum routers:
//!
//! ```rust,no_run
//! use autumn_web::security::SecurityHeadersLayer;
//! use autumn_web::security::HeadersConfig;
//!
//! let config = HeadersConfig::default();
//! let app = axum::Router::<()>::new()
//!     .route("/", axum::routing::get(|| async { "ok" }))
//!     .layer(SecurityHeadersLayer::from_config(&config));
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use axum::http::{HeaderValue, Request, Response, StatusCode};
use http::header::HeaderName;
use pin_project_lite::pin_project;
use tower::{Layer, Service};

use super::config::HeadersConfig;

/// Pre-computed header pairs to inject into every response.
///
/// Created once from [`HeadersConfig`] and shared via `Arc` across
/// all clones of [`SecurityHeadersService`].
#[derive(Debug, Clone)]
struct ComputedHeaders {
    pairs: Vec<(HeaderName, HeaderValue)>,
}

impl ComputedHeaders {
    fn from_config(config: &HeadersConfig) -> Self {
        let mut pairs = Vec::with_capacity(8);

        if !config.x_frame_options.is_empty()
            && let Ok(val) = HeaderValue::from_str(&config.x_frame_options)
        {
            pairs.push((HeaderName::from_static("x-frame-options"), val));
        }

        if config.x_content_type_options {
            pairs.push((
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ));
        }

        if config.xss_protection {
            pairs.push((
                HeaderName::from_static("x-xss-protection"),
                HeaderValue::from_static("1; mode=block"),
            ));
        }

        if config.strict_transport_security {
            let mut hsts = format!("max-age={}", config.hsts_max_age_secs);
            if config.hsts_include_subdomains {
                hsts.push_str("; includeSubDomains");
            }
            if let Ok(val) = HeaderValue::from_str(&hsts) {
                pairs.push((HeaderName::from_static("strict-transport-security"), val));
            }
        }

        if !config.content_security_policy.is_empty()
            && let Ok(val) = HeaderValue::from_str(&config.content_security_policy)
        {
            pairs.push((HeaderName::from_static("content-security-policy"), val));
        }

        if !config.referrer_policy.is_empty()
            && let Ok(val) = HeaderValue::from_str(&config.referrer_policy)
        {
            pairs.push((HeaderName::from_static("referrer-policy"), val));
        }

        if !config.permissions_policy.is_empty()
            && let Ok(val) = HeaderValue::from_str(&config.permissions_policy)
        {
            pairs.push((HeaderName::from_static("permissions-policy"), val));
        }

        Self { pairs }
    }
}

/// Tower [`Layer`] that adds security headers to every response.
///
/// Created from a [`HeadersConfig`] and applied to the Axum router.
/// The headers are pre-computed once and shared across all clones.
#[derive(Clone, Debug)]
pub struct SecurityHeadersLayer {
    headers: Arc<ComputedHeaders>,
    csp_nonce_enabled: bool,
    csp_nonce_directives: Vec<String>,
    base_csp: String,
}

impl SecurityHeadersLayer {
    /// Create a new layer from the given headers configuration.
    #[must_use]
    pub fn from_config(config: &HeadersConfig) -> Self {
        let is_default_csp =
            config.content_security_policy == super::config::default_content_security_policy();
        let csp_nonce_enabled = config.csp_nonce.enabled
            && !config.content_security_policy.is_empty()
            && is_default_csp;

        Self {
            headers: Arc::new(ComputedHeaders::from_config(config)),
            csp_nonce_enabled,
            csp_nonce_directives: config.csp_nonce.directives.clone(),
            base_csp: config.content_security_policy.clone(),
        }
    }
}

impl<S> Layer<S> for SecurityHeadersLayer {
    type Service = SecurityHeadersService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SecurityHeadersService {
            inner,
            headers: Arc::clone(&self.headers),
            csp_nonce_enabled: self.csp_nonce_enabled,
            csp_nonce_directives: self.csp_nonce_directives.clone(),
            base_csp: self.base_csp.clone(),
        }
    }
}

/// Tower [`Service`] produced by [`SecurityHeadersLayer`].
///
/// Adds pre-computed security headers to every HTTP response.
#[derive(Clone, Debug)]
pub struct SecurityHeadersService<S> {
    inner: S,
    headers: Arc<ComputedHeaders>,
    csp_nonce_enabled: bool,
    csp_nonce_directives: Vec<String>,
    base_csp: String,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for SecurityHeadersService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = SecurityHeadersFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let dynamic_nonce = if self.csp_nonce_enabled {
            let mut bytes = [0u8; 16];
            getrandom::getrandom(&mut bytes)
                .expect("failed to generate random bytes for CSP nonce");
            let nonce = base64_encode(&bytes);
            req.extensions_mut().insert(CspNonce(nonce.clone()));
            Some(nonce)
        } else {
            None
        };

        SecurityHeadersFuture {
            inner: self.inner.call(req),
            headers: Some(Arc::clone(&self.headers)),
            dynamic_nonce,
            csp_nonce_directives: self.csp_nonce_directives.clone(),
            base_csp: self.base_csp.clone(),
        }
    }
}

pin_project! {
    /// Future that injects security headers into the response.
    pub struct SecurityHeadersFuture<F> {
        #[pin]
        inner: F,
        headers: Option<Arc<ComputedHeaders>>,
        dynamic_nonce: Option<String>,
        csp_nonce_directives: Vec<String>,
        base_csp: String,
    }
}

impl<F, ResBody, E> Future for SecurityHeadersFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.inner.poll(cx) {
            Poll::Ready(Ok(mut response)) => {
                if let Some(computed) = this.headers.take() {
                    let resp_headers = response.headers_mut();
                    for (name, value) in &computed.pairs {
                        if name == "content-security-policy" && this.dynamic_nonce.is_some() {
                            continue;
                        }
                        resp_headers.insert(name.clone(), value.clone());
                    }
                    if let Some(nonce) = this.dynamic_nonce.take() {
                        let rewritten_csp =
                            inject_nonce_into_csp(this.base_csp, &nonce, this.csp_nonce_directives);
                        if let Ok(val) = HeaderValue::from_str(&rewritten_csp) {
                            resp_headers
                                .insert(HeaderName::from_static("content-security-policy"), val);
                        }
                    }
                }
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A Content-Security-Policy (CSP) nonce extracted from the request.
///
/// Use this as a handler parameter or in Maud templates to access the
/// per-request cryptographically secure random nonce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CspNonce(String);

impl CspNonce {
    /// Create a new `CspNonce`.
    #[must_use]
    pub const fn new(s: String) -> Self {
        Self(s)
    }

    /// Returns the CSP nonce string value.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CspNonce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<S> FromRequestParts<S> for CspNonce
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
            "CSP nonce not found in request extensions. Is SecurityHeadersLayer configured with csp_nonce enabled?",
        ))
    }
}

impl<S> OptionalFromRequestParts<S> for CspNonce
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

/// Helper function to base64 encode bytes.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for chunk in &mut chunks {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(n & 0x3F) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = u32::from(rem[0]) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = (u32::from(rem[0]) << 16) | (u32::from(rem[1]) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Injects the generated nonce into the specified directives of a CSP string.
pub fn inject_nonce_into_csp(csp: &str, nonce: &str, directives: &[String]) -> String {
    let parts: Vec<String> = csp
        .split(';')
        .map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                return String::new();
            }
            let mut words = trimmed.split_whitespace();
            words.next().map_or_else(
                || trimmed.to_owned(),
                |directive| {
                    if directives.iter().any(|d| d == directive) {
                        format!("{trimmed} 'nonce-{nonce}'")
                    } else {
                        trimmed.to_owned()
                    }
                },
            )
        })
        .filter(|s| !s.is_empty())
        .collect();

    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use std::task::{Context, Poll};
    use tower::{Layer, Service, ServiceExt};

    #[tokio::test]
    async fn default_headers_applied() {
        let config = HeadersConfig::default();
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.headers().get("x-frame-options").unwrap(), "DENY");
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(
            response.headers().get("x-xss-protection").unwrap(),
            "1; mode=block"
        );
        assert_eq!(
            response.headers().get("referrer-policy").unwrap(),
            "strict-origin-when-cross-origin"
        );
        // HSTS not present by default
        assert!(
            response
                .headers()
                .get("strict-transport-security")
                .is_none()
        );
        // CSP present by default with htmx-compatible policy
        let csp = response
            .headers()
            .get("content-security-policy")
            .expect("default CSP should be emitted");
        let csp = csp.to_str().unwrap();
        assert!(csp.contains("default-src 'self'"), "csp = {csp}");
        assert!(csp.contains("script-src 'self'"), "csp = {csp}");
        assert!(csp.contains("img-src 'self' data:"), "csp = {csp}");
        assert!(!csp.contains("'unsafe-eval'"), "csp = {csp}");
    }

    #[tokio::test]
    async fn default_csp_allows_htmx_to_function() {
        // htmx and Autumn's htmx CSRF helper are served from /static/js/
        // (same origin), operate via addEventListener, and issue hx-* requests
        // to the same origin. The default CSP must allow all of that without
        // requiring inline scripts.
        let config = HeadersConfig::default();
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let csp = response
            .headers()
            .get("content-security-policy")
            .expect("default CSP missing")
            .to_str()
            .unwrap()
            .to_owned();

        // htmx script loads from same origin
        assert!(
            csp.contains("script-src 'self'"),
            "csp must allow same-origin scripts for htmx: {csp}"
        );
        // htmx XHR/fetch requests go to same origin
        assert!(
            csp.contains("connect-src 'self'"),
            "csp must allow same-origin connects for htmx swaps: {csp}"
        );
        // No eval required for htmx standard operation
        assert!(
            !csp.contains("'unsafe-eval'"),
            "default csp should not weaken script-src with unsafe-eval: {csp}"
        );
    }

    #[tokio::test]
    async fn hsts_header_when_enabled() {
        let config = HeadersConfig {
            strict_transport_security: true,
            hsts_max_age_secs: 86400,
            hsts_include_subdomains: true,
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("strict-transport-security").unwrap(),
            "max-age=86400; includeSubDomains"
        );
    }

    #[tokio::test]
    async fn csp_header_when_configured() {
        let config = HeadersConfig {
            content_security_policy: "default-src 'self'".to_owned(),
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("content-security-policy").unwrap(),
            "default-src 'self'"
        );
    }

    #[tokio::test]
    async fn empty_x_frame_options_not_sent() {
        let config = HeadersConfig {
            x_frame_options: String::new(),
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(response.headers().get("x-frame-options").is_none());
    }

    #[tokio::test]
    async fn empty_csp_not_sent() {
        let config = HeadersConfig {
            content_security_policy: String::new(),
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(response.headers().get("content-security-policy").is_none());
    }

    #[tokio::test]
    async fn empty_referrer_policy_not_sent() {
        let config = HeadersConfig {
            referrer_policy: String::new(),
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(response.headers().get("referrer-policy").is_none());
    }

    #[tokio::test]
    async fn empty_permissions_policy_not_sent() {
        let config = HeadersConfig {
            permissions_policy: String::new(),
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(response.headers().get("permissions-policy").is_none());
    }

    #[tokio::test]
    async fn referrer_policy_when_configured() {
        let config = HeadersConfig {
            referrer_policy: "strict-origin-when-cross-origin".to_owned(),
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("referrer-policy").unwrap(),
            "strict-origin-when-cross-origin"
        );
    }

    #[tokio::test]
    async fn permissions_policy_when_configured() {
        let config = HeadersConfig {
            permissions_policy: "camera=(), microphone=()".to_owned(),
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("permissions-policy").unwrap(),
            "camera=(), microphone=()"
        );
    }

    #[tokio::test]
    async fn hsts_without_subdomains() {
        let config = HeadersConfig {
            strict_transport_security: true,
            hsts_max_age_secs: 3600,
            hsts_include_subdomains: false,
            ..Default::default()
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("strict-transport-security").unwrap(),
            "max-age=3600"
        );
    }

    #[derive(Clone)]
    struct PendingService;

    impl<ReqBody> Service<Request<ReqBody>> for PendingService {
        type Response = axum::response::Response;
        type Error = std::convert::Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }

        fn call(&mut self, _req: Request<ReqBody>) -> Self::Future {
            unreachable!("poll_ready should block this")
        }
    }

    #[test]
    fn layer_poll_ready_passes_through() {
        let layer = SecurityHeadersLayer::from_config(&HeadersConfig::default());
        let mut service = layer.layer(PendingService);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let poll = Service::<Request<axum::body::Body>>::poll_ready(&mut service, &mut cx);

        assert!(
            poll.is_pending(),
            "poll_ready should pass through Pending from inner service"
        );
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_inject_nonce_into_csp() {
        let base = "default-src 'self'; script-src 'self'; style-src 'self'";
        let directives = vec!["script-src".to_owned(), "style-src".to_owned()];
        let rewritten = inject_nonce_into_csp(base, "123456", &directives);
        assert_eq!(
            rewritten,
            "default-src 'self'; script-src 'self' 'nonce-123456'; style-src 'self' 'nonce-123456'"
        );
    }

    #[tokio::test]
    async fn middleware_generates_and_injects_nonce() {
        let config = HeadersConfig::default(); // csp_nonce is enabled by default!
        let app = Router::new()
            .route(
                "/",
                get(|nonce: CspNonce| async move { nonce.nonce().to_owned() }),
            )
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Assert CSP header contains this nonce (read headers first!)
        let csp = response
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();

        // Assert nonce is valid (base64, >= 128 bits i.e. >= 22 base64 chars without padding)
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let nonce_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(nonce_str.len() >= 22, "nonce: {nonce_str}");

        assert!(
            csp.contains(&format!("script-src 'self' 'nonce-{nonce_str}'")),
            "csp: {csp}"
        );
        assert!(
            csp.contains(&format!("style-src 'self' 'nonce-{nonce_str}'")),
            "csp: {csp}"
        );
    }

    #[tokio::test]
    async fn middleware_opt_out_custom_csp() {
        let config = HeadersConfig {
            content_security_policy: "default-src 'self'; script-src 'self'".to_owned(),
            ..Default::default()
        };
        let app = Router::new()
            .route(
                "/",
                get(|nonce: Option<CspNonce>| async move {
                    assert!(nonce.is_none());
                    "ok"
                }),
            )
            .layer(SecurityHeadersLayer::from_config(&config));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let csp = response
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(csp, "default-src 'self'; script-src 'self'");
    }
}
