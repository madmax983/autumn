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
//! Client IP is extracted from the connection peer address
//! ([`ConnectInfo<SocketAddr>`]) by default. When
//! [`RateLimitConfig::trust_forwarded_headers`] is `true` — which should
//! only be set behind a reverse proxy that strips and rewrites these
//! headers — the limiter consults `X-Forwarded-For` (first entry) and
//! `X-Real-IP` first, falling back to the peer address.
//!
//! Requests with no identifiable client (no trusted forwarding header
//! AND no `ConnectInfo`) bypass rate limiting entirely. In-process
//! callers such as the static site generator and test harnesses that
//! invoke the router via [`tower::ServiceExt::oneshot`] fall into this
//! bucket and must not be throttled.
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

use lru::LruCache;
use std::future::Future;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::Pin;

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

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
    trust_forwarded_headers: bool,
    buckets: Mutex<LruCache<String, Bucket>>,
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
        let burst_header = HeaderValue::from(config.burst.max(1));
        Self {
            refill_per_sec,
            burst,
            burst_header,
            trust_forwarded_headers: config.trust_forwarded_headers,
            buckets: Mutex::new(LruCache::new(
                NonZeroUsize::new(10_000).expect("10_000 is non-zero"),
            )),
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

            let mut bucket = buckets.get(key).copied().unwrap_or(Bucket {
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

            let result = if bucket.tokens >= 1.0 {
                bucket.tokens -= 1.0;
                Ok(bucket.tokens)
            } else {
                Err(bucket.tokens)
            };

            buckets.put(key.to_owned(), bucket);
            result
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
}

impl Limiter {
    /// Extract the originating client IP from a request, honoring this
    /// limiter's trusted-proxy policy.
    ///
    /// Returns `None` when no identifiable client is present, signalling
    /// the middleware to bypass throttling. In-process callers such as
    /// the static site generator and `Router::oneshot`-style test
    /// harnesses fall into this path.
    ///
    /// When `trust_forwarded_headers` is `true`, the first entry of
    /// `X-Forwarded-For` and then `X-Real-IP` are consulted before
    /// [`ConnectInfo<SocketAddr>`]. Otherwise only the peer address is
    /// used.
    fn client_ip<B>(&self, req: &Request<B>) -> Option<String> {
        if self.trust_forwarded_headers {
            let xff_ip = req
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit(',').next())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);

            if xff_ip.is_some() {
                return xff_ip;
            }

            let real_ip = req
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);

            if real_ip.is_some() {
                return real_ip;
            }
        }

        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip().to_string())
    }
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
        // When no identifiable client is present (no trusted forwarding
        // header, no ConnectInfo), bypass rate limiting. This covers
        // in-process callers like the static site generator and test
        // harnesses that invoke the router via `oneshot`.
        let decision = self
            .limiter
            .client_ip(&req)
            .map(|key| self.limiter.decide(&key, now));

        let limiter = Arc::clone(&self.limiter);
        let mut inner = self.inner.clone();
        // Swap to ensure correct poll_ready semantics.
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            match decision {
                Some(Decision::Denied { retry_after_secs }) => {
                    let mut response = Response::new(ResBody::default());
                    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                    let headers = response.headers_mut();
                    headers.insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
                    headers.insert(X_RATELIMIT_LIMIT, limiter.burst_header.clone());
                    headers.insert(X_RATELIMIT_REMAINING, HeaderValue::from_static("0"));
                    Ok(response)
                }
                Some(Decision::Allowed { remaining }) => {
                    let mut response = inner.call(req).await?;
                    let headers = response.headers_mut();
                    headers.insert(X_RATELIMIT_LIMIT, limiter.burst_header.clone());
                    headers.insert(X_RATELIMIT_REMAINING, HeaderValue::from(remaining));
                    Ok(response)
                }
                None => inner.call(req).await,
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
    use std::time::Duration;
    use tower::ServiceExt;

    fn cfg(enabled: bool, rps: f64, burst: u32) -> RateLimitConfig {
        RateLimitConfig {
            enabled,
            requests_per_second: rps,
            burst,
            // Tests exercise the key-by-IP path via X-Forwarded-For so
            // they don't need a real TCP listener.
            trust_forwarded_headers: true,
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
            .expect("infallible response builder")
    }

    fn limiter(trust: bool) -> Limiter {
        Limiter::from_config(&RateLimitConfig {
            enabled: true,
            requests_per_second: 10.0,
            burst: 5,
            trust_forwarded_headers: trust,
        })
    }

    #[tokio::test]
    async fn requests_under_limit_pass() {
        let app = app(&cfg(true, 1.0, 5));
        for _ in 0..5 {
            let response = app
                .clone()
                .oneshot(req_with_ip("1.1.1.1"))
                .await
                .expect("infallible response builder");
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().get("x-ratelimit-limit").is_some());
        }
    }

    #[tokio::test]
    async fn request_over_limit_returns_429_with_retry_after() {
        let app = app(&cfg(true, 1.0, 2));

        // Burn through the burst.
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(req_with_ip("2.2.2.2"))
                .await
                .expect("infallible response builder");
            assert_eq!(response.status(), StatusCode::OK);
        }

        // Next request is over the limit.
        let response = app
            .clone()
            .oneshot(req_with_ip("2.2.2.2"))
            .await
            .expect("infallible response builder");
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .expect("Retry-After header present")
            .to_str()
            .expect("infallible response builder")
            .parse::<u64>()
            .expect("Retry-After parses as integer seconds");
        assert!(retry_after >= 1);

        assert_eq!(
            response
                .headers()
                .get("x-ratelimit-remaining")
                .expect("infallible response builder")
                .to_str()
                .expect("infallible response builder"),
            "0"
        );
    }

    #[tokio::test]
    async fn different_ips_are_independent() {
        let app = app(&cfg(true, 0.1, 1));

        // Exhaust IP A.
        let ok_a = app
            .clone()
            .oneshot(req_with_ip("10.0.0.1"))
            .await
            .expect("infallible response builder");
        assert_eq!(ok_a.status(), StatusCode::OK);
        let blocked_a = app
            .clone()
            .oneshot(req_with_ip("10.0.0.1"))
            .await
            .expect("infallible response builder");
        assert_eq!(blocked_a.status(), StatusCode::TOO_MANY_REQUESTS);

        // IP B still has a full bucket.
        let ok_b = app
            .clone()
            .oneshot(req_with_ip("10.0.0.2"))
            .await
            .expect("infallible response builder");
        assert_eq!(ok_b.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tokens_refill_over_time() {
        let app = app(&cfg(true, 50.0, 1));

        let first = app
            .clone()
            .oneshot(req_with_ip("3.3.3.3"))
            .await
            .expect("infallible response builder");
        assert_eq!(first.status(), StatusCode::OK);
        let blocked = app
            .clone()
            .oneshot(req_with_ip("3.3.3.3"))
            .await
            .expect("infallible response builder");
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);

        // Wait long enough for one token to refill (50 rps -> 20ms per token).
        tokio::time::sleep(Duration::from_millis(80)).await;

        let after_refill = app
            .clone()
            .oneshot(req_with_ip("3.3.3.3"))
            .await
            .expect("infallible response builder");
        assert_eq!(after_refill.status(), StatusCode::OK);
    }

    #[test]
    fn client_ip_prefers_x_forwarded_for_rightmost_entry_when_trusted() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "1.2.3.4, 5.6.7.8")
            .body(())
            .expect("infallible response builder");
        assert_eq!(limiter(true).client_ip(&req).as_deref(), Some("5.6.7.8"));
    }

    #[test]
    fn client_ip_trims_whitespace_when_trusted() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "  9.9.9.9  ")
            .body(())
            .expect("infallible response builder");
        assert_eq!(limiter(true).client_ip(&req).as_deref(), Some("9.9.9.9"));
    }

    #[test]
    fn client_ip_falls_back_to_x_real_ip_when_trusted() {
        let req: Request<()> = Request::builder()
            .header("X-Real-IP", "7.7.7.7")
            .body(())
            .expect("infallible response builder");
        assert_eq!(limiter(true).client_ip(&req).as_deref(), Some("7.7.7.7"));
    }

    #[test]
    fn client_ip_falls_back_to_connect_info() {
        let mut req: Request<()> = Request::builder()
            .body(())
            .expect("infallible response builder");
        let addr: SocketAddr = "127.0.0.1:4242"
            .parse()
            .expect("infallible response builder");
        req.extensions_mut().insert(ConnectInfo(addr));
        assert_eq!(limiter(true).client_ip(&req).as_deref(), Some("127.0.0.1"));
        // Untrusted limiter also falls back, since headers are ignored.
        assert_eq!(limiter(false).client_ip(&req).as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn client_ip_none_when_no_source() {
        // In-process callers without ConnectInfo (SSG, tests) must be
        // bypassed, not collapsed onto a shared fallback bucket.
        let req: Request<()> = Request::builder()
            .body(())
            .expect("infallible response builder");
        assert!(limiter(true).client_ip(&req).is_none());
        assert!(limiter(false).client_ip(&req).is_none());
    }

    #[test]
    fn client_ip_empty_xff_falls_through_to_x_real_ip_when_trusted() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", " , 5.5.5.5")
            .header("X-Real-IP", "8.8.8.8")
            .body(())
            .expect("infallible response builder");
        // Rightmost XFF entry is valid, we don't fall back to X-Real-IP.
        assert_eq!(limiter(true).client_ip(&req).as_deref(), Some("5.5.5.5"));
    }

    #[test]
    fn client_ip_ignores_forwarded_headers_by_default() {
        // Attacker-supplied forwarding headers must not be trusted when
        // `trust_forwarded_headers = false`; the limiter keys on the
        // real peer address instead.
        let mut req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "1.2.3.4")
            .header("X-Real-IP", "5.6.7.8")
            .body(())
            .expect("infallible response builder");
        let addr: SocketAddr = "10.0.0.42:1111"
            .parse()
            .expect("infallible response builder");
        req.extensions_mut().insert(ConnectInfo(addr));
        assert_eq!(limiter(false).client_ip(&req).as_deref(), Some("10.0.0.42"));
    }

    #[tokio::test]
    async fn forwarded_headers_cannot_bypass_throttling_when_untrusted() {
        // `trust_forwarded_headers = false` (default). When ConnectInfo is
        // set (the production configuration), rotating XFF values must
        // NOT shard the throttle into separate buckets — the peer IP is
        // the sole key source.
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0.1,
            burst: 1,
            trust_forwarded_headers: false,
        };
        let app = app(&config);
        let peer: SocketAddr = "198.51.100.1:2000"
            .parse()
            .expect("infallible response builder");

        let make_req = |xff: &str| {
            let mut req = Request::builder()
                .method("GET")
                .uri("/")
                .header("X-Forwarded-For", xff)
                .body(Body::empty())
                .expect("infallible response builder");
            req.extensions_mut().insert(ConnectInfo(peer));
            req
        };

        let first = app
            .clone()
            .oneshot(make_req("1.1.1.1"))
            .await
            .expect("infallible response builder");
        assert_eq!(first.status(), StatusCode::OK);
        // Different XFF value, but same peer → still throttled.
        let blocked = app
            .clone()
            .oneshot(make_req("2.2.2.2"))
            .await
            .expect("infallible response builder");
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn requests_without_connect_info_bypass_rate_limit() {
        // Static site generation and `Router::oneshot`-style callers
        // don't set ConnectInfo. The limiter must pass them through so
        // build-time rendering isn't throttled onto a shared bucket.
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0.001,
            burst: 1,
            trust_forwarded_headers: false,
        };
        let app = app(&config);

        for _ in 0..10 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/")
                        .body(Body::empty())
                        .expect("infallible response builder"),
                )
                .await
                .expect("infallible response builder");
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response.headers().get("x-ratelimit-limit").is_none(),
                "bypassed requests should not carry rate-limit headers"
            );
        }
    }
}
