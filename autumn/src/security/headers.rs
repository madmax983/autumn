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
//! use autumn_web::security::headers::SecurityHeadersLayer;
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

use axum::http::{HeaderValue, Request, Response};
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

        if !config.x_frame_options.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&config.x_frame_options) {
                pairs.push((HeaderName::from_static("x-frame-options"), val));
            }
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

        if !config.content_security_policy.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&config.content_security_policy) {
                pairs.push((HeaderName::from_static("content-security-policy"), val));
            }
        }

        if !config.referrer_policy.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&config.referrer_policy) {
                pairs.push((HeaderName::from_static("referrer-policy"), val));
            }
        }

        if !config.permissions_policy.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&config.permissions_policy) {
                pairs.push((HeaderName::from_static("permissions-policy"), val));
            }
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
}

impl SecurityHeadersLayer {
    /// Create a new layer from the given headers configuration.
    #[must_use]
    pub fn from_config(config: &HeadersConfig) -> Self {
        Self {
            headers: Arc::new(ComputedHeaders::from_config(config)),
        }
    }
}

impl<S> Layer<S> for SecurityHeadersLayer {
    type Service = SecurityHeadersService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SecurityHeadersService {
            inner,
            headers: Arc::clone(&self.headers),
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

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        SecurityHeadersFuture {
            inner: self.inner.call(req),
            headers: Some(Arc::clone(&self.headers)),
        }
    }
}

pin_project! {
    /// Future that injects security headers into the response.
    pub struct SecurityHeadersFuture<F> {
        #[pin]
        inner: F,
        headers: Option<Arc<ComputedHeaders>>,
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
                        resp_headers.insert(name.clone(), value.clone());
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
}
