//! Maintenance mode Tower middleware.
//!
//! Intercepts HTTP requests when maintenance mode is active and returns
//! `503 Service Unavailable` with a `Retry-After` header. Routes under the
//! configured actuator/health prefix always pass through so platform load
//! balancers keep every replica in rotation.
//!
//! # Response format negotiation
//!
//! - Requests with `Accept: text/html` (or no `Accept`) receive an HTML page.
//! - Requests with `Accept: application/json` or
//!   `Accept: application/problem+json` receive an RFC 7807 Problem Details
//!   JSON object.
//!
//! # Bypass mechanisms
//!
//! In order of evaluation:
//!
//! 1. **Actuator prefix** – paths starting with `health_prefix` (default
//!    `/actuator`) always pass through.
//! 2. **Bypass header** – a configured `(header_name, expected_value)` pair;
//!    requests carrying the matching header are not gated.
//! 3. **IP allow-list** – CIDR-expanded list; clients whose IP matches any
//!    entry bypass the 503.
//! 4. **Read-only mode** – when `readonly = true`, `GET`, `HEAD`, and
//!    `OPTIONS` requests pass through; write methods are gated.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{
    Method, Request, Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
};
use pin_project_lite::pin_project;
use tower::{Layer, Service};

use crate::maintenance::{MaintenanceConfig, MaintenanceState, ip_in_allow_list};

/// Default health/actuator prefix whose routes bypass maintenance.
pub const DEFAULT_HEALTH_PREFIX: &str = "/actuator";

/// Retry-After value (seconds) sent in every 503 response.
const RETRY_AFTER_SECS: &str = "120";

/// Tower [`Layer`] that adds maintenance-mode gating to a service.
///
/// Clone this layer and call [`with_health_prefix`](Self::with_health_prefix)
/// to override the default `/actuator` bypass prefix.
#[derive(Clone)]
pub struct MaintenanceLayer {
    state: MaintenanceState,
    health_prefix: String,
    trust_forwarded_headers: bool,
    trusted_proxies: Vec<crate::security::TrustedProxy>,
}

impl MaintenanceLayer {
    /// Create a [`MaintenanceLayer`] backed by `state`.
    ///
    /// Uses `/actuator` as the health-check prefix by default.
    #[must_use]
    pub fn new(state: MaintenanceState) -> Self {
        Self {
            state,
            health_prefix: DEFAULT_HEALTH_PREFIX.to_owned(),
            trust_forwarded_headers: false,
            trusted_proxies: Vec::new(),
        }
    }

    /// Override the health-check prefix (e.g. `/health`).
    ///
    /// Requests to paths that start with this prefix always pass through,
    /// regardless of maintenance state, so load balancers keep the replica
    /// in rotation.
    #[must_use]
    pub fn with_health_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.health_prefix = prefix.into();
        self
    }

    /// Set whether to trust forwarded headers like `X-Forwarded-For`.
    #[must_use]
    pub const fn with_trust_forwarded_headers(mut self, trust: bool) -> Self {
        self.trust_forwarded_headers = trust;
        self
    }

    /// Configure the trusted proxies list.
    #[must_use]
    pub fn with_trusted_proxies(mut self, proxies: Vec<crate::security::TrustedProxy>) -> Self {
        self.trusted_proxies = proxies;
        self
    }
}

impl<S> Layer<S> for MaintenanceLayer {
    type Service = MaintenanceService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MaintenanceService {
            inner,
            state: self.state.clone(),
            health_prefix: self.health_prefix.clone(),
            trust_forwarded_headers: self.trust_forwarded_headers,
            trusted_proxies: self.trusted_proxies.clone(),
        }
    }
}

/// Tower [`Service`] produced by [`MaintenanceLayer`].
#[derive(Clone)]
pub struct MaintenanceService<S> {
    inner: S,
    state: MaintenanceState,
    health_prefix: String,
    trust_forwarded_headers: bool,
    trusted_proxies: Vec<crate::security::TrustedProxy>,
}

impl<S> MaintenanceService<S> {
    /// Determine whether this request should be gated by maintenance mode.
    ///
    /// Returns `Some(503 response)` when the request should be blocked, or
    /// `None` when it should pass through to the inner service.
    fn gate_request<B>(
        &self,
        req: &Request<B>,
        config: &MaintenanceConfig,
    ) -> Option<Response<Body>> {
        // 1. Actuator/health routes always pass through.
        if req.uri().path().starts_with(self.health_prefix.as_str()) {
            return None;
        }

        // 2. Bypass header.
        if let Some((header_name, expected_value)) = &config.bypass_header
            && let Some(val) = req.headers().get(header_name.as_str())
            && val.as_bytes() == expected_value.as_bytes()
        {
            return None;
        }

        // 3. IP allow-list.
        if !config.allow_ips.is_empty()
            && let Some(client_ip) = extract_client_ip(req, self.trust_forwarded_headers, &self.trusted_proxies)
            && ip_in_allow_list(&client_ip, &config.allow_ips)
        {
            return None;
        }

        // 4. Read-only mode: safe methods pass through.
        if config.readonly {
            let method = req.method();
            if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
                return None;
            }
        }

        Some(build_503_response(req, config))
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for MaintenanceService<S>
where
    S: Service<Request<ReqBody>, Response = Response<Body>>,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = MaintenanceFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        if let Some(config) = self.state.get()
            && let Some(response) = self.gate_request(&req, &config)
        {
            return MaintenanceFuture::ShortCircuit {
                response: Some(response),
            };
        }
        MaintenanceFuture::Forward {
            inner: self.inner.call(req),
        }
    }
}

pin_project! {
    /// Future returned by [`MaintenanceService`].
    ///
    /// Either resolves immediately with a 503 response (short-circuit path)
    /// or delegates to the wrapped inner service.
    #[project = MaintenanceFutureProj]
    pub enum MaintenanceFuture<F> {
        ShortCircuit { response: Option<Response<Body>> },
        Forward { #[pin] inner: F },
    }
}

impl<F, E> Future for MaintenanceFuture<F>
where
    F: Future<Output = Result<Response<Body>, E>>,
{
    type Output = Result<Response<Body>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            MaintenanceFutureProj::ShortCircuit { response } => Poll::Ready(Ok(response
                .take()
                .expect("MaintenanceFuture polled after completion"))),
            MaintenanceFutureProj::Forward { inner } => inner.poll(cx),
        }
    }
}

/// Extract the client IP from a request, preferring proxy-forwarded headers.
fn extract_client_ip<B>(
    req: &Request<B>,
    trust_forwarded_headers: bool,
    trusted_proxies: &[crate::security::TrustedProxy],
) -> Option<IpAddr> {
    let configured = !trusted_proxies.is_empty();
    crate::security::proxy::extract_client_ip(
        req,
        trust_forwarded_headers,
        trusted_proxies,
        configured,
    )
}

/// Build a 503 response with the appropriate content type.
fn build_503_response<B>(req: &Request<B>, config: &MaintenanceConfig) -> Response<Body> {
    let message = config
        .message
        .as_deref()
        .unwrap_or("The service is temporarily unavailable. Please try again later.");

    let wants_json = req
        .headers()
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| {
            accept.contains("application/json") || accept.contains("application/problem+json")
        });

    if wants_json {
        let body = serde_json::json!({
            "type": "about:blank",
            "title": "Service Unavailable",
            "status": 503,
            "detail": message,
        });
        Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", RETRY_AFTER_SECS)
            .header(CONTENT_TYPE, "application/problem+json")
            .body(Body::from(body.to_string()))
            .expect("valid 503 JSON response")
    } else {
        let html = format!(
            "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"UTF-8\">\
             <title>503 Service Unavailable</title>\
             <style>body{{font-family:sans-serif;max-width:600px;margin:4rem auto;padding:0 1rem}}\
             h1{{color:#c0392b}}</style></head>\
             <body><h1>Service Temporarily Unavailable</h1>\
             <p>{message}</p>\
             <p>Please try again later.</p></body></html>"
        );
        Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", RETRY_AFTER_SECS)
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(html))
            .expect("valid 503 HTML response")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maintenance::MaintenanceConfig;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt; // for oneshot

    fn make_app(state: MaintenanceState) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .route("/api/data", get(|| async { "data" }))
            .route("/actuator/health", get(|| async { "healthy" }))
            .layer(MaintenanceLayer::new(state).with_trust_forwarded_headers(true))
    }

    async fn response_status(app: Router, uri: &str) -> StatusCode {
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    // ── Maintenance off ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_off_passes_through() {
        let state = MaintenanceState::new();
        let app = make_app(state);
        assert_eq!(response_status(app, "/").await, StatusCode::OK);
    }

    // ── Maintenance on — basic 503 ────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_on_returns_503() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());
        let app = make_app(state);
        assert_eq!(
            response_status(app, "/").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn maintenance_on_includes_retry_after_header() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());
        let app = make_app(state);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(resp.headers().contains_key("retry-after"));
    }

    // ── Content negotiation ───────────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_on_html_response_for_browser() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(ACCEPT, "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let ct = resp.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(ct.contains("text/html"), "expected text/html, got {ct}");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Service Temporarily Unavailable"),
            "body: {html}"
        );
    }

    #[tokio::test]
    async fn maintenance_on_json_response_for_api_client() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/data")
                    .header(ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let ct = resp.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(
            ct.contains("application/problem+json"),
            "expected problem+json, got {ct}"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], 503);
        assert_eq!(json["title"], "Service Unavailable");
        assert!(json["detail"].is_string(), "detail should be a string");
    }

    #[tokio::test]
    async fn maintenance_on_problem_json_for_problem_json_accept() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/data")
                    .header(ACCEPT, "application/problem+json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let ct = resp.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(ct.contains("application/problem+json"), "got {ct}");
    }

    #[tokio::test]
    async fn maintenance_on_custom_message_in_body() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            message: Some("Deploying v2.0".into()),
            ..Default::default()
        });
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(ACCEPT, "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Deploying v2.0"),
            "custom message absent: {html}"
        );
    }

    // ── Actuator / health bypass ──────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_on_actuator_path_passes_through() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());
        let app = make_app(state);
        assert_eq!(
            response_status(app, "/actuator/health").await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn maintenance_on_custom_health_prefix_passes_through() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());

        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/", get(|| async { "root" }))
            .layer(MaintenanceLayer::new(state).with_health_prefix("/health"));

        assert_eq!(
            response_status(app.clone(), "/health").await,
            StatusCode::OK
        );
        assert_eq!(
            response_status(app, "/").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    // ── Bypass header ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_on_bypass_header_passes_through() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            bypass_header: Some(("X-Autumn-Maintenance-Bypass".into(), "my-secret".into())),
            ..Default::default()
        });
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("X-Autumn-Maintenance-Bypass", "my-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn maintenance_on_wrong_bypass_header_value_blocked() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            bypass_header: Some(("X-Autumn-Maintenance-Bypass".into(), "my-secret".into())),
            ..Default::default()
        });
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("X-Autumn-Maintenance-Bypass", "wrong-value")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn maintenance_on_missing_bypass_header_blocked() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            bypass_header: Some(("X-Autumn-Maintenance-Bypass".into(), "my-secret".into())),
            ..Default::default()
        });
        let app = make_app(state);
        assert_eq!(
            response_status(app, "/").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    // ── IP allow-list bypass ──────────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_on_allowed_ip_passes_through() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            allow_ips: vec!["127.0.0.1".into()],
            ..Default::default()
        });
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("X-Forwarded-For", "127.0.0.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn maintenance_on_disallowed_ip_blocked() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            allow_ips: vec!["10.0.0.0/8".into()],
            ..Default::default()
        });
        let app = make_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("X-Forwarded-For", "192.168.1.5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ── Read-only mode ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_readonly_get_passes_through() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            readonly: true,
            ..Default::default()
        });
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(MaintenanceLayer::new(state));

        assert_eq!(response_status(app, "/").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn maintenance_readonly_post_returns_503() {
        use axum::routing::post;
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            readonly: true,
            ..Default::default()
        });
        let app = Router::new()
            .route("/submit", post(|| async { "ok" }))
            .layer(MaintenanceLayer::new(state));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/submit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn maintenance_readonly_head_passes_through() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig {
            readonly: true,
            ..Default::default()
        });
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(MaintenanceLayer::new(state));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // HEAD returns 200 (no body)
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Layer behaviour ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn maintenance_layer_clone_shares_state() {
        let state = MaintenanceState::new();
        let layer = MaintenanceLayer::new(state.clone());

        // The cloned layer wraps the same underlying Arc, so enabling
        // maintenance through `state` is visible through the clone.
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(layer.clone());

        state.enable(MaintenanceConfig::default());
        assert_eq!(
            response_status(app, "/").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
