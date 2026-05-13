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
//! headers — the limiter consults `X-Forwarded-For` and `X-Real-IP` first,
//! falling back to the peer address. If
//! [`RateLimitConfig::trusted_proxies`] is configured, trusted proxy
//! addresses at the right side of the `X-Forwarded-For` chain are skipped,
//! but only when the request peer address is present and trusted, so
//! appended proxy chains still key on the nearest untrusted client without
//! trusting spoofable headers from direct callers.
//!
//! Requests with no identifiable client (no trusted forwarding header
//! AND no `ConnectInfo`) bypass rate limiting entirely. In-process
//! callers such as the static site generator and test harnesses that
//! invoke the router via [`tower::ServiceExt::oneshot`] fall into this
//! bucket and must not be throttled.
//!
//! # Backends
//!
//! The bucket store is configurable via [`RateLimitConfig::backend`]:
//!
//! - `"memory"` (default): in-process LRU of token buckets. Zero-config for
//!   development. Each replica enforces the limit independently, so a
//!   3-replica deployment permits up to 3× the configured rate.
//! - `"redis"`: shared bucket store coordinated across replicas via an atomic
//!   Lua script. The configured rate is enforced globally regardless of replica
//!   count. Reuses the same Redis connection as sessions, cache, and the
//!   scheduler.
//!
//! When the Redis backend is unavailable, behavior is controlled by
//! [`RateLimitConfig::on_backend_failure`]:
//! - `"fail_open"` (default): allow the request through, matching the
//!   existing single-replica posture.
//! - `"fail_closed"`: return `429` until the backend recovers.
//!
//! A single `tracing::warn!` is emitted per outage, not per request.
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
//!
//! # Multi-replica: share the budget across all pods
//! backend = "redis"
//! on_backend_failure = "fail_open"
//!
//! [security.rate_limit.redis]
//! url = "redis://redis:6379"
//! key_prefix = "myapp:rate_limit"
//! ```

use lru::LruCache;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
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
#[cfg(feature = "redis")]
use super::config::{RateLimitBackend, RateLimitBackendFailure};

const X_RATELIMIT_LIMIT: HeaderName = HeaderName::from_static("x-ratelimit-limit");
const X_RATELIMIT_REMAINING: HeaderName = HeaderName::from_static("x-ratelimit-remaining");

/// Outcome of consuming one token from a bucket.
#[derive(Debug, Clone, Copy)]
enum Decision {
    Allowed { remaining: u32 },
    Denied { retry_after_secs: u64 },
}

// ── In-memory bucket store ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Per-IP token bucket state stored in-process.
#[derive(Debug)]
struct MemoryStore {
    buckets: Mutex<LruCache<String, Bucket>>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            buckets: Mutex::new(LruCache::new(
                NonZeroUsize::new(10_000).expect("10_000 is non-zero"),
            )),
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    fn decide(&self, key: &str, now: Instant, burst: f64, refill_per_sec: f64) -> Decision {
        let tokens_after = {
            let mut buckets = match self.buckets.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };

            let mut bucket = buckets.get(key).copied().unwrap_or(Bucket {
                tokens: burst,
                last_refill: now,
            });

            let elapsed = now
                .saturating_duration_since(bucket.last_refill)
                .as_secs_f64();
            bucket.tokens = elapsed.mul_add(refill_per_sec, bucket.tokens).min(burst);
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
                let secs = (deficit / refill_per_sec).ceil().max(1.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let retry_after_secs = secs as u64;
                Decision::Denied { retry_after_secs }
            }
        }
    }
}

// ── Redis bucket store ────────────────────────────────────────────────────────

#[cfg(feature = "redis")]
const RATE_LIMIT_LUA: &str = include_str!("rate_limit.lua");

#[cfg(feature = "redis")]
struct RedisStore {
    connection: redis::aio::ConnectionManager,
    key_prefix: String,
    failure_mode: RateLimitBackendFailure,
    /// Set to `true` once on the first Redis error; reset when it recovers.
    outage_logged: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "redis")]
impl std::fmt::Debug for RedisStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisStore")
            .field("key_prefix", &self.key_prefix)
            .field("failure_mode", &self.failure_mode)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "redis")]
impl RedisStore {
    const fn new(
        connection: redis::aio::ConnectionManager,
        key_prefix: String,
        failure_mode: RateLimitBackendFailure,
    ) -> Self {
        Self {
            connection,
            key_prefix,
            failure_mode,
            outage_logged: std::sync::atomic::AtomicBool::new(false),
        }
    }

    async fn decide(&self, key: &str, burst: f64, refill_per_sec: f64) -> Option<Decision> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let redis_key = format!("{}:{}", self.key_prefix, key);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let script = redis::Script::new(RATE_LIMIT_LUA);
        let result: redis::RedisResult<Vec<i64>> = {
            let mut conn = self.connection.clone();
            script
                .key(&redis_key)
                .arg(burst)
                .arg(refill_per_sec)
                .arg(i64::try_from(now_ms).unwrap_or(i64::MAX))
                .invoke_async(&mut conn)
                .await
        };

        match result {
            Ok(values) if values.len() == 3 => {
                // Redis recovered after an outage — reset the flag so we warn again next time.
                self.outage_logged
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                let allowed = values[0] == 1;
                if allowed {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let remaining = values[1] as u32;
                    Some(Decision::Allowed { remaining })
                } else {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let retry_after_secs = values[2].max(1) as u64;
                    Some(Decision::Denied { retry_after_secs })
                }
            }
            Err(err) => {
                // Emit one warning per outage, not per request.
                if !self
                    .outage_logged
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    tracing::warn!(
                        error = %err,
                        key_prefix = %self.key_prefix,
                        "rate-limit Redis backend unavailable; \
                         switching to on_backend_failure posture"
                    );
                }
                match self.failure_mode {
                    RateLimitBackendFailure::FailOpen => None,
                    RateLimitBackendFailure::FailClosed => Some(Decision::Denied {
                        retry_after_secs: 1,
                    }),
                }
            }
            Ok(values) => {
                // Unexpected script return shape — treat as backend error.
                if !self
                    .outage_logged
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    tracing::warn!(
                        ?values,
                        key_prefix = %self.key_prefix,
                        "rate-limit Redis backend: unexpected script return value; \
                         switching to on_backend_failure posture"
                    );
                }
                match self.failure_mode {
                    RateLimitBackendFailure::FailOpen => None,
                    RateLimitBackendFailure::FailClosed => Some(Decision::Denied {
                        retry_after_secs: 1,
                    }),
                }
            }
        }
    }
}

// ── Backend enum ──────────────────────────────────────────────────────────────

#[derive(Debug)]
enum BucketBackend {
    Memory(MemoryStore),
    #[cfg(feature = "redis")]
    Redis(RedisStore),
}

// ── Limiter (shared state) ────────────────────────────────────────────────────

/// Shared rate limiter state.
#[derive(Debug)]
struct Limiter {
    refill_per_sec: f64,
    burst: f64,
    burst_header: HeaderValue,
    trust_forwarded_headers: bool,
    trusted_proxies_configured: bool,
    trusted_proxies: Vec<TrustedProxy>,
    backend: BucketBackend,
}

#[derive(Debug, Clone, Copy)]
struct TrustedProxy {
    network: IpAddr,
    prefix_len: u8,
}

impl TrustedProxy {
    fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        let (addr, prefix_len) = if let Some((addr, prefix)) = trimmed.split_once('/') {
            let addr = addr.trim().parse::<IpAddr>().ok()?;
            let prefix_len = prefix.trim().parse::<u8>().ok()?;
            (addr, prefix_len)
        } else {
            let addr = trimmed.parse::<IpAddr>().ok()?;
            let prefix_len = match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            };
            (addr, prefix_len)
        };

        let max_prefix = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };

        (prefix_len <= max_prefix).then_some(Self {
            network: addr,
            prefix_len,
        })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        if self.prefix_len == 0 {
            return matches!(
                (self.network, ip),
                (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
            );
        }

        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                let shift = 32_u8.saturating_sub(self.prefix_len);
                (u32::from(network) >> shift) == (u32::from(candidate) >> shift)
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                let shift = 128_u8.saturating_sub(self.prefix_len);
                (u128::from(network) >> shift) == (u128::from(candidate) >> shift)
            }
            (IpAddr::V4(_), IpAddr::V6(_)) | (IpAddr::V6(_), IpAddr::V4(_)) => false,
        }
    }
}

impl Limiter {
    fn from_config(config: &RateLimitConfig) -> Self {
        let burst = f64::from(config.burst.max(1));
        let refill_per_sec = config.requests_per_second.max(f64::MIN_POSITIVE);
        let burst_header = HeaderValue::from(config.burst.max(1));
        let trusted_proxies_configured = !config.trusted_proxies.is_empty();
        let trusted_proxies = config
            .trusted_proxies
            .iter()
            .filter_map(|proxy| {
                TrustedProxy::parse(proxy).or_else(|| {
                    tracing::warn!(
                        trusted_proxy = %proxy,
                        "ignoring invalid rate limit trusted proxy"
                    );
                    None
                })
            })
            .collect();

        let backend = Self::build_backend(config);

        Self {
            refill_per_sec,
            burst,
            burst_header,
            trust_forwarded_headers: config.trust_forwarded_headers,
            trusted_proxies_configured,
            trusted_proxies,
            backend,
        }
    }

    fn build_backend(#[allow(unused_variables)] config: &RateLimitConfig) -> BucketBackend {
        #[cfg(feature = "redis")]
        if config.backend == RateLimitBackend::Redis {
            if let Some(url) = config.redis.url.as_deref().filter(|u| !u.trim().is_empty()) {
                match redis::Client::open(url) {
                    Ok(client) => {
                        match redis::aio::ConnectionManager::new_lazy_with_config(
                            client,
                            redis::aio::ConnectionManagerConfig::new(),
                        ) {
                            Ok(conn) => {
                                return BucketBackend::Redis(RedisStore::new(
                                    conn,
                                    config.redis.key_prefix.clone(),
                                    config.on_backend_failure,
                                ));
                            }
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    url = %url,
                                    "rate-limit Redis backend: failed to create \
                                     connection manager; falling back to memory"
                                );
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            url = %url,
                            "rate-limit Redis backend: invalid Redis URL; \
                             falling back to memory"
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "rate-limit backend = \"redis\" but no redis.url configured; \
                     falling back to memory"
                );
            }
        }
        BucketBackend::Memory(MemoryStore::new())
    }

    /// Consume one token for `key`. Returns `None` when throttling must be bypassed
    /// (no client, or Redis fail-open).
    async fn decide(&self, key: &str) -> Option<Decision> {
        match &self.backend {
            BucketBackend::Memory(store) => {
                Some(store.decide(key, Instant::now(), self.burst, self.refill_per_sec))
            }
            #[cfg(feature = "redis")]
            BucketBackend::Redis(store) => store.decide(key, self.burst, self.refill_per_sec).await,
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
    /// When `trust_forwarded_headers` is `true`, `X-Forwarded-For` and
    /// then `X-Real-IP` are consulted before [`ConnectInfo<SocketAddr>`].
    /// If trusted proxies are configured, the `X-Forwarded-For` chain is
    /// walked from right to left and configured proxy IPs/CIDRs are
    /// skipped, but only when the request peer is present and trusted.
    /// Without a trusted proxy list, the last non-empty XFF entry is used
    /// for the existing single-proxy setup where proxies append the peer
    /// address after any client-supplied header value. If that entry is
    /// the immediate socket peer, the peer has appended its own address,
    /// so the client entry immediately to its left is used instead.
    fn client_ip<B>(&self, req: &Request<B>) -> Option<String> {
        let peer_ip = Self::peer_ip(req);

        if self.trust_forwarded_headers && self.forwarded_headers_allowed(req) {
            let xff_ip = req
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| self.client_ip_from_x_forwarded_for(s, peer_ip));

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

        peer_ip.map(|ip| ip.to_string())
    }

    fn peer_ip<B>(req: &Request<B>) -> Option<IpAddr> {
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip())
    }

    fn forwarded_headers_allowed<B>(&self, req: &Request<B>) -> bool {
        if !self.trusted_proxies_configured {
            return true;
        }

        Self::peer_ip(req).is_some_and(|peer_ip| self.is_trusted_proxy(peer_ip))
    }

    fn client_ip_from_x_forwarded_for(
        &self,
        header: &str,
        peer_ip: Option<IpAddr>,
    ) -> Option<String> {
        if !self.trusted_proxies_configured {
            let mut entries = header.rsplit(',').map(str::trim).filter(|s| !s.is_empty());
            let last = entries.next()?;

            if peer_ip.is_some_and(|peer_ip| last.parse::<IpAddr>().is_ok_and(|ip| ip == peer_ip)) {
                return entries
                    .next()
                    .map_or_else(|| Some(last.to_owned()), |entry| Some(entry.to_owned()));
            }

            return Some(last.to_owned());
        }

        for entry in header.rsplit(',').map(str::trim).filter(|s| !s.is_empty()) {
            let Ok(ip) = entry.parse::<IpAddr>() else {
                continue;
            };

            if !self.is_trusted_proxy(ip) {
                return Some(ip.to_string());
            }
        }

        None
    }

    fn is_trusted_proxy(&self, ip: IpAddr) -> bool {
        self.trusted_proxies
            .iter()
            .any(|trusted_proxy| trusted_proxy.contains(ip))
    }
}

// ── Tower layer & service ─────────────────────────────────────────────────────

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
        // Extract the client key synchronously (no I/O required).
        let client_key = self.limiter.client_ip(&req);

        let limiter = Arc::clone(&self.limiter);
        let mut inner = self.inner.clone();
        // Swap to ensure correct poll_ready semantics.
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            // Resolve the rate-limit decision (may be async for the Redis backend).
            let decision = match client_key {
                Some(key) => limiter.decide(&key).await,
                None => None,
            };

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
            trusted_proxies: Vec::new(),
            ..Default::default()
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
            trusted_proxies: Vec::new(),
            ..Default::default()
        })
    }

    fn limiter_with_trusted_proxies(proxies: &[&str]) -> Limiter {
        Limiter::from_config(&RateLimitConfig {
            enabled: true,
            requests_per_second: 10.0,
            burst: 5,
            trust_forwarded_headers: true,
            trusted_proxies: proxies.iter().map(|proxy| (*proxy).to_owned()).collect(),
            ..Default::default()
        })
    }

    fn req_with_connect_info(xff: &str, peer: &str) -> Request<()> {
        let mut req: Request<()> = Request::builder()
            .header("X-Forwarded-For", xff)
            .body(())
            .expect("infallible response builder");
        let addr: SocketAddr = peer.parse().expect("test peer socket address parses");
        req.extensions_mut().insert(ConnectInfo(addr));
        req
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
    fn client_ip_uses_proxy_appended_entry_without_proxy_list() {
        let req = req_with_connect_info("attacker_spoofed_ip, 198.51.100.77", "203.0.113.10:4000");
        assert_eq!(
            limiter(true).client_ip(&req).as_deref(),
            Some("198.51.100.77")
        );
    }

    #[test]
    fn client_ip_prefers_x_forwarded_for_proxy_appended_entry_without_proxy_list() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "1.2.3.4, 5.6.7.8")
            .body(())
            .expect("infallible response builder");
        assert_eq!(limiter(true).client_ip(&req).as_deref(), Some("5.6.7.8"));
    }

    #[test]
    fn client_ip_uses_rightmost_forwarded_entry_without_configured_proxy_list() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "198.51.100.77, 203.0.113.10")
            .body(())
            .expect("infallible response builder");

        assert_eq!(
            limiter(true).client_ip(&req).as_deref(),
            Some("203.0.113.10")
        );
    }

    #[test]
    fn client_ip_skips_peer_self_append_without_configured_proxy_list() {
        let req = req_with_connect_info("198.51.100.77, 203.0.113.10", "203.0.113.10:4000");

        assert_eq!(
            limiter(true).client_ip(&req).as_deref(),
            Some("198.51.100.77")
        );
    }

    #[test]
    fn client_ip_skips_configured_trusted_proxy_chain_entries() {
        let req = req_with_connect_info("198.51.100.77, 203.0.113.10", "203.0.113.10:4000");
        assert_eq!(
            limiter_with_trusted_proxies(&["203.0.113.10"])
                .client_ip(&req)
                .as_deref(),
            Some("198.51.100.77")
        );
    }

    #[test]
    fn client_ip_skips_configured_trusted_proxy_cidr_entries() {
        let req = req_with_connect_info("198.51.100.77, 203.0.113.10, 10.0.0.5", "10.0.0.5:4000");
        assert_eq!(
            limiter_with_trusted_proxies(&["203.0.113.0/24", "10.0.0.5"])
                .client_ip(&req)
                .as_deref(),
            Some("198.51.100.77")
        );
    }

    #[test]
    fn client_ip_accepts_forwarded_chain_from_configured_trusted_peer() {
        let req = req_with_connect_info("198.51.100.77, 203.0.113.10", "10.0.0.5:4000");

        assert_eq!(
            limiter_with_trusted_proxies(&["10.0.0.5", "203.0.113.10"])
                .client_ip(&req)
                .as_deref(),
            Some("198.51.100.77")
        );
    }

    #[test]
    fn client_ip_ignores_forwarded_chain_from_untrusted_peer() {
        let req = req_with_connect_info("198.51.100.77, 203.0.113.10", "192.0.2.44:4000");

        assert_eq!(
            limiter_with_trusted_proxies(&["203.0.113.10"])
                .client_ip(&req)
                .as_deref(),
            Some("192.0.2.44")
        );
    }

    #[test]
    fn client_ip_ignores_forwarded_headers_without_peer_when_trusted_proxies_configured() {
        let req: Request<()> = Request::builder()
            .header("X-Forwarded-For", "198.51.100.77, 203.0.113.10")
            .header("X-Real-IP", "198.51.100.88")
            .body(())
            .expect("infallible response builder");

        assert!(
            limiter_with_trusted_proxies(&["203.0.113.10"])
                .client_ip(&req)
                .is_none()
        );
    }

    #[test]
    fn client_ip_falls_back_to_peer_when_all_trusted_proxies_are_invalid() {
        let req = req_with_connect_info("198.51.100.77, 203.0.113.10", "192.0.2.44:4000");

        assert_eq!(
            limiter_with_trusted_proxies(&["203.0.113.10:443", "198.51.100.0/999"])
                .client_ip(&req)
                .as_deref(),
            Some("192.0.2.44")
        );
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
            .header("X-Forwarded-For", "  ")
            .header("X-Real-IP", "8.8.8.8")
            .body(())
            .expect("infallible response builder");
        // The XFF client entry is empty after trim, so we fall back to X-Real-IP.
        assert_eq!(limiter(true).client_ip(&req).as_deref(), Some("8.8.8.8"));
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
            trusted_proxies: Vec::new(),
            ..Default::default()
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
    async fn forwarded_header_chains_keep_independent_client_buckets() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0.1,
            burst: 1,
            trust_forwarded_headers: true,
            trusted_proxies: vec!["203.0.113.10".to_owned()],
            ..Default::default()
        };
        let app = app(&config);

        let req_with_chain = |client_ip: &str| {
            let mut req = Request::builder()
                .method("GET")
                .uri("/")
                .header("X-Forwarded-For", format!("{client_ip}, 203.0.113.10"))
                .body(Body::empty())
                .expect("infallible response builder");
            let peer: SocketAddr = "203.0.113.10:4000"
                .parse()
                .expect("test peer socket address parses");
            req.extensions_mut().insert(ConnectInfo(peer));
            req
        };

        let first_a = app
            .clone()
            .oneshot(req_with_chain("198.51.100.77"))
            .await
            .expect("infallible response builder");
        assert_eq!(first_a.status(), StatusCode::OK);

        let blocked_a = app
            .clone()
            .oneshot(req_with_chain("198.51.100.77"))
            .await
            .expect("infallible response builder");
        assert_eq!(blocked_a.status(), StatusCode::TOO_MANY_REQUESTS);

        let first_b = app
            .clone()
            .oneshot(req_with_chain("198.51.100.88"))
            .await
            .expect("infallible response builder");
        assert_eq!(first_b.status(), StatusCode::OK);
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
            trusted_proxies: Vec::new(),
            ..Default::default()
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

    #[tokio::test]
    async fn requests_without_connect_info_bypass_when_trusted_proxies_configured() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0.001,
            burst: 1,
            trust_forwarded_headers: true,
            trusted_proxies: vec!["203.0.113.10".to_owned()],
            ..Default::default()
        };
        let app = app(&config);

        for _ in 0..3 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/")
                        .header("X-Forwarded-For", "198.51.100.77, 203.0.113.10")
                        .header("X-Real-IP", "198.51.100.88")
                        .body(Body::empty())
                        .expect("infallible response builder"),
                )
                .await
                .expect("infallible response builder");
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response.headers().get("x-ratelimit-limit").is_none(),
                "requests with configured trusted proxies but no peer must not trust forwarded headers"
            );
        }
    }

    #[tokio::test]
    async fn invalid_trusted_proxies_do_not_reopen_forwarded_header_trust() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0.001,
            burst: 1,
            trust_forwarded_headers: true,
            trusted_proxies: vec!["203.0.113.10:443".to_owned(), "198.51.100.0/999".to_owned()],
            ..Default::default()
        };
        let app = app(&config);
        let peer: SocketAddr = "192.0.2.44:4000"
            .parse()
            .expect("test peer socket address parses");

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
            .oneshot(make_req("198.51.100.77"))
            .await
            .expect("infallible response builder");
        assert_eq!(first.status(), StatusCode::OK);

        let blocked = app
            .clone()
            .oneshot(make_req("198.51.100.88"))
            .await
            .expect("infallible response builder");
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
