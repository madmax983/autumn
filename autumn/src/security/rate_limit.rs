//! Rate limiting middleware.
//!
//! Protects endpoints from abuse by applying a per-client-IP token bucket.
//! Requests that exhaust their bucket receive `429 Too Many Requests` with
//! a `Retry-After` header indicating when to retry.
//!
//! # How it works
//!
//! Each client IP gets its own token bucket holding up to `burst` tokens
//! that refill at `requests_per_second`. Every incoming request costs one
//! token. When the bucket is empty the middleware rejects the request
//! without invoking the handler.
//!
//! Client IP is extracted (in order) from:
//!
//! 1. The first entry of the `X-Forwarded-For` header.
//! 2. The `X-Real-IP` header.
//! 3. The connection peer address (via `ConnectInfo<SocketAddr>`).
//!
//! # Configuration
//!
//! See [`RateLimitConfig`] for available settings.
//!
//! ```toml
//! [security.rate_limit]
//! enabled = true
//! requests_per_second = 10.0
//! burst = 20
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::extract::ConnectInfo;
use axum::http::{HeaderValue, Request, Response, StatusCode};
use http::header::{HeaderName, RETRY_AFTER};
use tower::{Layer, Service};

use super::config::RateLimitConfig;

const X_RATELIMIT_LIMIT: HeaderName = HeaderName::from_static("x-ratelimit-limit");
const X_RATELIMIT_REMAINING: HeaderName = HeaderName::from_static("x-ratelimit-remaining");

/// Shared rate limiter state.
#[derive(Debug)]
struct Limiter {
    refill_per_sec: f64,
    burst: f64,
    burst_header: HeaderValue,
    buckets: Mutex<HashMap<String, Bucket>>,
    calls: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Outcome of consuming one token from a bucket.
#[derive(Debug, Clone, Copy)]
enum Decision {
    Allowed { remaining: u32 },
    Denied { retry_after_secs: u64 },
}

impl Limiter {
    fn from_config(config: &RateLimitConfig) -> Self {
        let burst = f64::from(config.burst.max(1));
        let refill_per_sec = config.requests_per_second.max(f64::MIN_POSITIVE);
        let burst_header = HeaderValue::from_str(&config.burst.max(1).to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0"));
        Self {
            refill_per_sec,
            burst,
            burst_header,
            buckets: Mutex::new(HashMap::new()),
            calls: AtomicU64::new(0),
        }
    }

    #[allow(clippy::significant_drop_tightening)] // lock protects the bucket mutation
    fn decide(&self, key: &str, now: Instant) -> Decision {
        // Scope the lock guard so it's released before we produce the final
        // `Decision`. Keeps the critical section tight.
        let tokens_after = {
            let mut buckets = match self.buckets.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let bucket = buckets.entry(key.to_owned()).or_insert(Bucket {
                tokens: self.burst,
                last_refill: now,
            });

            let elapsed = now
                .saturating_duration_since(bucket.last_refill)
                .as_secs_f64();
            bucket.tokens = elapsed
                .mul_add(self.refill_per_sec, bucket.tokens)
                .min(self.burst);
            bucket.last_refill = now;

            if bucket.tokens >= 1.0 {
                bucket.tokens -= 1.0;
                Ok(bucket.tokens)
            } else {
                Err(bucket.tokens)
            }
        };

        match tokens_after {
            Ok(remaining_tokens) => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let remaining = remaining_tokens.floor() as u32;
                Decision::Allowed { remaining }
            }
            Err(current_tokens) => {
                let deficit = 1.0 - current_tokens;
                let secs = (deficit / self.refill_per_sec).ceil().max(1.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let retry_after_secs = secs as u64;
                Decision::Denied { retry_after_secs }
            }
        }
    }

    /// Probabilistic eviction of long-idle buckets to bound memory usage.
    fn maybe_sweep(&self, now: Instant) {
        // Sweep roughly once per 1024 calls. Cheap, branchless, no background task.
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        if n & 0x3FF != 0 {
            return;
        }
        let Ok(mut buckets) = self.buckets.lock() else {
            return;
        };
        let idle_cutoff = Duration::from_secs(300);
        buckets.retain(|_, b| {
            now.saturating_duration_since(b.last_refill) < idle_cutoff || b.tokens < self.burst
        });
    }
}

/// Extract the originating client IP from a request.
///
/// Consults (in order) `X-Forwarded-For`, `X-Real-IP`, and the socket
/// peer address recorded via [`ConnectInfo`]. Returns the literal
/// string `"unknown"` when no source is available.
pub fn client_ip<B>(req: &Request<B>) -> String {
    if let Some(value) = req.headers().get("x-forwarded-for") {
        if let Ok(s) = value.to_str() {
            if let Some(first) = s.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_owned();
                }
            }
        }
    }

    if let Some(value) = req.headers().get("x-real-ip") {
        if let Ok(s) = value.to_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }

    if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return addr.ip().to_string();
    }

    "unknown".to_owned()
}

/// Tower [`Layer`] that applies rate limiting.
///
/// Applied automatically when `security.rate_limit.enabled = true`.
#[derive(Clone, Debug)]
pub struct RateLimitLayer {
    limiter: Arc<Limiter>,
}

impl RateLimitLayer {
    /// Create a new rate limit layer from configuration.
    #[must_use]
    pub fn from_config(config: &RateLimitConfig) -> Self {
        Self {
            limiter: Arc::new(Limiter::from_config(config)),
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: Arc::clone(&self.limiter),
        }
    }
}

/// Tower [`Service`] produced by [`RateLimitLayer`].
#[derive(Clone, Debug)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: Arc<Limiter>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RateLimitService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let now = Instant::now();
        let key = client_ip(&req);
        let decision = self.limiter.decide(&key, now);
        self.limiter.maybe_sweep(now);

        let limiter = Arc::clone(&self.limiter);
        let mut inner = self.inner.clone();
        // Swap to ensure correct poll_ready semantics.
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            match decision {
                Decision::Denied { retry_after_secs } => {
                    let mut response = Response::new(ResBody::default());
                    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                    let headers = response.headers_mut();
                    if let Ok(v) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                        headers.insert(RETRY_AFTER, v);
                    }
                    headers.insert(X_RATELIMIT_LIMIT, limiter.burst_header.clone());
                    headers.insert(X_RATELIMIT_REMAINING, HeaderValue::from_static("0"));
                    Ok(response)
                }
                Decision::Allowed { remaining } => {
                    let mut response = inner.call(req).await?;
                    let headers = response.headers_mut();
                    headers.insert(X_RATELIMIT_LIMIT, limiter.burst_header.clone());
                    if let Ok(v) = HeaderValue::from_str(&remaining.to_string()) {
                        headers.insert(X_RATELIMIT_REMAINING, v);
                    }
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
    use axum::routing::get;
    use tower::ServiceExt;

    fn cfg(enabled: bool, rps: f64, burst: u32) -> RateLimitConfig {
        RateLimitConfig {
            enabled,
            requests_per_second: rps,
            burst,
        }
    }

    fn app(config: &RateLimitConfig) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(RateLimitLayer::from_config(config))
    }

    fn req_with_ip(ip: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri("/")
            .header("X-Forwarded-For", ip)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn requests_under_limit_pass() {
        let app = app(&cfg(true, 1.0, 5));
        for _ in 0..5 {
            let response = app.clone().oneshot(req_with_ip("1.1.1.1")).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().get("x-ratelimit-limit").is_some());
        }
    }

    #[tokio::test]
    async fn request_over_limit_returns_429_with_retry_after() {
        let app = app(&cfg(true, 1.0, 2));

        // Burn through the burst.
        for _ in 0..2 {
            let response = app.clone().oneshot(req_with_ip("2.2.2.2")).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        // Next request is over the limit.
        let response = app.clone().oneshot(req_with_ip("2.2.2.2")).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .expect("Retry-After header present")
            .to_str()
            .unwrap()
            .parse::<u64>()
            .expect("Retry-After parses as integer seconds");
        assert!(retry_after >= 1);

        assert_eq!(
            response
                .headers()
                .get("x-ratelimit-remaining")
                .unwrap()
                .to_str()
                .unwrap(),
            "0"
        );
    }

    #[tokio::test]
    async fn different_ips_are_independent() {
        let app = app(&cfg(true, 0.1, 1));

        // Exhaust IP A.
        let ok_a = app.clone().oneshot(req_with_ip("10.0.0.1")).await.unwrap();
        assert_eq!(ok_a.status(), StatusCode::OK);
        let blocked_a = app.clone().oneshot(req_with_ip("10.0.0.1")).await.unwrap();
        assert_eq!(blocked_a.status(), StatusCode::TOO_MANY_REQUESTS);

        // IP B still has a full bucket.
        let ok_b = app.clone().oneshot(req_with_ip("10.0.0.2")).await.unwrap();
        assert_eq!(ok_b.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tokens_refill_over_time() {
        let app = app(&cfg(true, 50.0, 1));

        let first = app.clone().oneshot(req_with_ip("3.3.3.3")).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let blocked = app.clone().oneshot(req_with_ip("3.3.3.3")).await.unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);

        // Wait long enough for one token to refill (50 rps -> 20ms per token).
        tokio::time::sleep(Duration::from_millis(80)).await;

        let after_refill = app.clone().oneshot(req_with_ip("3.3.3.3")).await.unwrap();
        assert_eq!(after_refill.status(), StatusCode::OK);
    }

    #[test]
    fn client_ip_prefers_x_forwarded_for_first_entry() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "1.2.3.4, 5.6.7.8")
            .body(())
            .unwrap();
        assert_eq!(client_ip(&req), "1.2.3.4");
    }

    #[test]
    fn client_ip_trims_whitespace() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "  9.9.9.9  ")
            .body(())
            .unwrap();
        assert_eq!(client_ip(&req), "9.9.9.9");
    }

    #[test]
    fn client_ip_falls_back_to_x_real_ip() {
        let req: Request<()> = Request::builder()
            .header("X-Real-IP", "7.7.7.7")
            .body(())
            .unwrap();
        assert_eq!(client_ip(&req), "7.7.7.7");
    }

    #[test]
    fn client_ip_falls_back_to_connect_info() {
        let mut req: Request<()> = Request::builder().body(()).unwrap();
        let addr: SocketAddr = "127.0.0.1:4242".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        assert_eq!(client_ip(&req), "127.0.0.1");
    }

    #[test]
    fn client_ip_unknown_when_no_source() {
        let req: Request<()> = Request::builder().body(()).unwrap();
        assert_eq!(client_ip(&req), "unknown");
    }

    #[test]
    fn client_ip_empty_xff_falls_through() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", " , 5.5.5.5")
            .header("X-Real-IP", "8.8.8.8")
            .body(())
            .unwrap();
        // First XFF entry is empty after trim, so we fall back to X-Real-IP.
        assert_eq!(client_ip(&req), "8.8.8.8");
    }
}
