use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, Response, StatusCode};
use tower::{Layer, Service};

static IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
static X_IDEMPOTENT_REPLAYED: &str = "x-idempotent-replayed";

/// How long an in-flight marker survives before being treated as stale.
/// Guards against crashes that leave the lock permanently held.
const IN_FLIGHT_TTL: Duration = Duration::from_secs(30);

/// Maximum response body size buffered for caching. Responses larger than this
/// return 502 rather than exhaust server memory.
const MAX_RESPONSE_BODY_SIZE: usize = 10 * 1024 * 1024; // 10 MiB

/// Maximum request body size read for hash comparison. Larger bodies get 413.
const MAX_REQUEST_BODY_SIZE: usize = 10 * 1024 * 1024; // 10 MiB

const fn is_mutating_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn compute_body_hash(bytes: &[u8]) -> Vec<u8> {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish().to_le_bytes().to_vec()
}

fn extract_replay_headers(headers: &HeaderMap) -> Vec<(String, Vec<u8>)> {
    // Headers that must not be cached or replayed.
    // `set-cookie` is excluded to prevent session fixation and replay of expired tokens.
    const SKIP: &[&str] = &[
        "connection",
        "transfer-encoding",
        "keep-alive",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "set-cookie",
        "x-idempotent-replayed",
    ];
    headers
        .iter()
        .filter(|(name, _)| !SKIP.contains(&name.as_str()))
        .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
        .collect()
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Stored response associated with an idempotency key.
#[derive(Clone)]
pub struct IdempotencyRecord {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
}

/// Cache entry wrapping a record with expiry and request body fingerprint.
#[derive(Clone)]
pub struct IdempotencyEntry {
    pub record: IdempotencyRecord,
    pub body_hash: Vec<u8>,
    pub expires_at: Instant,
}

// ── Store trait ───────────────────────────────────────────────────────────────

/// Pluggable storage backend for idempotency entries.
///
/// Implementors must be `Send + Sync + 'static` to be used across async tasks.
/// All methods are synchronous; long-running I/O backends should use
/// [`tokio::task::block_in_place`] internally.
pub trait IdempotencyStore: Send + Sync + 'static {
    /// Return the cached entry if it exists and has not expired.
    fn get(&self, key: &str) -> Option<IdempotencyEntry>;

    /// Persist a response with the given TTL.
    fn set(&self, key: &str, record: IdempotencyRecord, body_hash: Vec<u8>, ttl: Duration);

    /// Acquire an in-flight lock for `key`.
    ///
    /// Returns `true` if the lock was acquired (no concurrent request in flight)
    /// or `false` if another request is already processing this key.
    fn try_lock(&self, key: &str) -> bool;

    /// Release the in-flight lock for `key`.
    fn unlock(&self, key: &str);
}

// ── Memory store ──────────────────────────────────────────────────────────────

/// In-memory idempotency store backed by a `RwLock<HashMap>`.
///
/// Evicts expired entries lazily on `get`. In-flight markers are evicted
/// lazily per-key on `try_lock` using a 30-second stale threshold.
///
/// Suitable for single-process deployments and integration tests. For
/// multi-replica deployments configure `backend = "redis"` in `autumn.toml`.
pub struct MemoryIdempotencyStore {
    entries: RwLock<HashMap<String, IdempotencyEntry>>,
    in_flight: RwLock<HashMap<String, Instant>>,
    /// Counts `set` calls to trigger periodic expired-entry eviction.
    write_count: AtomicU64,
}

impl MemoryIdempotencyStore {
    #[must_use]
    pub fn new(_default_ttl: Duration) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            in_flight: RwLock::new(HashMap::new()),
            write_count: AtomicU64::new(0),
        }
    }
}

impl IdempotencyStore for MemoryIdempotencyStore {
    fn get(&self, key: &str) -> Option<IdempotencyEntry> {
        // Release the read lock immediately after cloning.
        let entry = self.entries.read().unwrap().get(key).cloned();
        entry.filter(|e| e.expires_at > Instant::now())
    }

    fn set(&self, key: &str, record: IdempotencyRecord, body_hash: Vec<u8>, ttl: Duration) {
        let entry = IdempotencyEntry {
            record,
            body_hash,
            expires_at: Instant::now() + ttl,
        };
        let mut entries = self.entries.write().unwrap();
        entries.insert(key.to_owned(), entry);
        // Periodically evict expired entries to bound memory growth for
        // long-running processes. O(N) scan is amortised over every 128 writes.
        let n = self.write_count.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(128) {
            let now = Instant::now();
            entries.retain(|_, v| v.expires_at > now);
        }
    }

    fn try_lock(&self, key: &str) -> bool {
        let mut in_flight = self.in_flight.write().unwrap();
        let now = Instant::now();
        // Check only the requested key's lock for staleness — avoids O(N) retain
        // on every lock acquisition under high concurrency.
        if in_flight
            .get(key)
            .is_some_and(|&started_at| now.duration_since(started_at) < IN_FLIGHT_TTL)
        {
            return false; // still in flight
        }
        // Not locked, or lock is stale (handler crashed) — acquire.
        in_flight.insert(key.to_owned(), now);
        true
    }

    fn unlock(&self, key: &str) {
        self.in_flight.write().unwrap().remove(key);
    }
}

// ── Redis store ───────────────────────────────────────────────────────────────

#[cfg(feature = "redis")]
mod redis_store {
    use super::{IdempotencyEntry, IdempotencyRecord, IdempotencyStore};
    use redis::{AsyncCommands, Client, aio::ConnectionManager, aio::ConnectionManagerConfig};
    use serde::{Deserialize, Serialize};
    use std::time::{Duration, Instant};

    const LOCK_TTL_SECS: u64 = 30;

    #[derive(Serialize, Deserialize)]
    struct StoredEntry {
        status: u16,
        headers: Vec<(String, Vec<u8>)>,
        body: Vec<u8>,
        body_hash: Vec<u8>,
    }

    /// Redis-backed idempotency store for multi-replica deployments.
    ///
    /// Configured via `[idempotency.redis]` in `autumn.toml` or
    /// `AUTUMN_IDEMPOTENCY__REDIS__URL` env var.
    pub struct RedisIdempotencyStore {
        connection: ConnectionManager,
        key_prefix: String,
    }

    impl RedisIdempotencyStore {
        /// Creates a [`RedisIdempotencyStore`] from the application idempotency config.
        ///
        /// # Errors
        /// Returns an error string if no Redis URL is configured or if the Redis
        /// client cannot be opened.
        pub fn from_config(config: &crate::config::IdempotencyConfig) -> Result<Self, String> {
            let url = config
                .redis
                .url
                .as_deref()
                .filter(|u| !u.trim().is_empty())
                .ok_or_else(|| {
                    "Redis idempotency backend requires a URL. \
                     Set AUTUMN_IDEMPOTENCY__REDIS__URL or \
                     [idempotency.redis] url in autumn.toml."
                        .to_owned()
                })?;
            let client = Client::open(url).map_err(|e| e.to_string())?;
            let connection =
                ConnectionManager::new_lazy_with_config(client, ConnectionManagerConfig::new())
                    .map_err(|e| e.to_string())?;
            Ok(Self {
                connection,
                key_prefix: config.redis.key_prefix.clone(),
            })
        }

        fn entry_key(&self, key: &str) -> String {
            format!("{}:entry:{}", self.key_prefix, key)
        }

        fn lock_key(&self, key: &str) -> String {
            format!("{}:lock:{}", self.key_prefix, key)
        }
    }

    impl IdempotencyStore for RedisIdempotencyStore {
        fn get(&self, key: &str) -> Option<IdempotencyEntry> {
            let redis_key = self.entry_key(key);
            let mut conn = self.connection.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    let data: Option<Vec<u8>> = conn.get(&redis_key).await.ok().flatten();
                    data.and_then(|bytes| {
                        serde_json::from_slice::<StoredEntry>(&bytes).ok().map(|e| {
                            IdempotencyEntry {
                                record: IdempotencyRecord {
                                    status: e.status,
                                    headers: e.headers,
                                    body: e.body,
                                },
                                body_hash: e.body_hash,
                                // Redis manages TTL natively. Use a fixed 24 h offset
                                // so the in-process expiry check never fires early.
                                expires_at: Instant::now() + Duration::from_secs(86_400),
                            }
                        })
                    })
                })
            })
        }

        fn set(&self, key: &str, record: IdempotencyRecord, body_hash: Vec<u8>, ttl: Duration) {
            let redis_key = self.entry_key(key);
            let mut conn = self.connection.clone();
            let entry = StoredEntry {
                status: record.status,
                headers: record.headers,
                body: record.body,
                body_hash,
            };
            if let Ok(bytes) = serde_json::to_vec(&entry) {
                let ttl_secs = ttl.as_secs().max(1);
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        if let Err(e) = conn.set_ex::<_, _, ()>(&redis_key, bytes, ttl_secs).await {
                            // The handler already succeeded. Log and continue so
                            // the response is returned; a retry will re-execute
                            // the handler (idempotency guarantee is degraded).
                            tracing::warn!(
                                error = %e,
                                "Failed to persist idempotency entry to Redis; \
                                 a retry may re-execute the handler"
                            );
                        }
                    });
                });
            }
        }

        fn try_lock(&self, key: &str) -> bool {
            let lock_key = self.lock_key(key);
            let mut conn = self.connection.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    let result: Result<Option<String>, _> = redis::cmd("SET")
                        .arg(&lock_key)
                        .arg("1")
                        .arg("NX")
                        .arg("EX")
                        .arg(LOCK_TTL_SECS)
                        .query_async(&mut conn)
                        .await;
                    match result {
                        Ok(opt) => opt.is_some(), // Some("OK") = acquired, None = already held
                        Err(e) => {
                            // Redis unavailable: degrade gracefully — allow the request
                            // through rather than returning 409 for every request.
                            tracing::warn!(
                                error = %e,
                                "Redis idempotency lock unavailable; \
                                 allowing request through (degraded mode)"
                            );
                            true
                        }
                    }
                })
            })
        }

        fn unlock(&self, key: &str) {
            let lock_key = self.lock_key(key);
            let mut conn = self.connection.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    let _: Result<(), _> = conn.del(&lock_key).await;
                });
            });
        }
    }
}

#[cfg(feature = "redis")]
pub use redis_store::RedisIdempotencyStore;

// ── Layer ─────────────────────────────────────────────────────────────────────

/// Tower [`Layer`] that enforces HTTP idempotency semantics per IETF
/// `draft-ietf-httpapi-idempotency-key-header`.
///
/// Applies only to mutating HTTP methods (POST, PUT, PATCH, DELETE).
/// Requests without an `Idempotency-Key` header are passed through unchanged.
///
/// - **Cache hit, same body**: replays the stored response with
///   `X-Idempotent-Replayed: true` and skips the handler.
/// - **Cache hit, different body**: returns `422 Unprocessable Entity`.
/// - **Concurrent duplicate** (same key, already in flight): returns
///   `409 Conflict` with `Retry-After: 1`.
/// - **Cache miss**: forwards to the handler, stores the response.
#[derive(Clone)]
pub struct IdempotencyLayer {
    store: Arc<dyn IdempotencyStore>,
    ttl: Duration,
    metrics: Option<crate::middleware::MetricsCollector>,
}

impl IdempotencyLayer {
    #[must_use]
    pub fn new(store: Arc<dyn IdempotencyStore>) -> Self {
        Self {
            store,
            ttl: Duration::from_secs(86_400),
            metrics: None,
        }
    }

    #[must_use]
    pub const fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    #[must_use]
    pub fn with_metrics(mut self, metrics: crate::middleware::MetricsCollector) -> Self {
        self.metrics = Some(metrics);
        self
    }
}

impl<S> Layer<S> for IdempotencyLayer {
    type Service = IdempotencyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        IdempotencyService {
            inner,
            store: self.store.clone(),
            ttl: self.ttl,
            metrics: self.metrics.clone(),
        }
    }
}

// ── Service ───────────────────────────────────────────────────────────────────

/// Tower [`Service`] produced by [`IdempotencyLayer`].
#[derive(Clone)]
pub struct IdempotencyService<S> {
    inner: S,
    store: Arc<dyn IdempotencyStore>,
    ttl: Duration,
    metrics: Option<crate::middleware::MetricsCollector>,
}

impl<S> Service<Request<Body>> for IdempotencyService<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = std::convert::Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = std::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let inner = self.inner.clone();
        let store = self.store.clone();
        let ttl = self.ttl;
        let metrics = self.metrics.clone();
        Box::pin(handle_idempotent_request(inner, store, ttl, metrics, req))
    }
}

async fn handle_idempotent_request<S>(
    mut inner: S,
    store: Arc<dyn IdempotencyStore>,
    ttl: Duration,
    metrics: Option<crate::middleware::MetricsCollector>,
    req: Request<Body>,
) -> Result<Response<Body>, std::convert::Infallible>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = std::convert::Infallible>
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    if !is_mutating_method(req.method()) {
        return inner.call(req).await;
    }

    let idempotency_key = match req.headers().get(IDEMPOTENCY_KEY_HEADER) {
        Some(v) => v.to_str().unwrap_or("").to_owned(),
        None => return inner.call(req).await,
    };

    if idempotency_key.is_empty() {
        return inner.call(req).await;
    }

    let (parts, body) = req.into_parts();

    // Return 413 if the request body exceeds the hash-comparison limit.
    let Ok(body_bytes) = axum::body::to_bytes(body, MAX_REQUEST_BODY_SIZE).await else {
        let response = Response::builder()
            .status(StatusCode::PAYLOAD_TOO_LARGE)
            .body(Body::from(
                "request body too large for idempotency middleware",
            ))
            .unwrap();
        return Ok(response);
    };

    let body_hash = compute_body_hash(&body_bytes);

    // ── Cache hit ──────────────────────────────────────────────────────────
    if let Some(entry) = store.get(&idempotency_key) {
        return Ok(handle_cache_hit(
            entry,
            &body_hash,
            &idempotency_key,
            metrics.as_ref(),
        ));
    }

    // ── In-flight check (concurrent duplicate) ─────────────────────────────
    if !store.try_lock(&idempotency_key) {
        tracing::debug!(
            idempotency.key = %idempotency_key,
            "Idempotency key already in flight — returning 409"
        );
        if let Some(m) = &metrics {
            m.record_idempotency_conflict();
        }
        let response = Response::builder()
            .status(StatusCode::CONFLICT)
            .header("retry-after", "1")
            .body(Body::from(
                "a request with this idempotency key is already being processed; \
                 retry after 1 second",
            ))
            .unwrap();
        return Ok(response);
    }

    // ── Cache miss: forward to handler, then cache the result ──────────────
    tracing::debug!(
        idempotency.key = %idempotency_key,
        idempotency.replayed = false,
        "Idempotency cache miss — forwarding to handler"
    );

    let req = Request::from_parts(parts, Body::from(body_bytes));
    let response = inner.call(req).await?;
    let (resp_parts, resp_body) = response.into_parts();
    let status = resp_parts.status.as_u16();

    // Buffer the response body. If it exceeds the size cap the body is consumed
    // and cannot be reconstituted, so return the response with the correct
    // status and original headers but an empty body — better than a synthetic
    // 502 that masks a successful handler operation and triggers a retry.
    let Ok(resp_bytes) = axum::body::to_bytes(resp_body, MAX_RESPONSE_BODY_SIZE).await else {
        store.unlock(&idempotency_key);
        tracing::warn!(
            idempotency.key = %idempotency_key,
            limit_bytes = MAX_RESPONSE_BODY_SIZE,
            "Response body exceeded cache size limit; not storing idempotency entry"
        );
        return Ok(Response::from_parts(resp_parts, Body::empty()));
    };

    // Filtered headers for the cache entry: set-cookie is excluded to prevent
    // session fixation when the stored response is replayed to a later caller.
    // The immediate (first) response uses the original resp_parts and therefore
    // delivers set-cookie normally.
    let replay_headers = extract_replay_headers(&resp_parts.headers);

    // Only cache successful responses (2xx) so transient errors can be retried.
    if (200u32..300).contains(&u32::from(status)) {
        let record = IdempotencyRecord {
            status,
            headers: replay_headers,
            body: resp_bytes.to_vec(),
        };
        store.set(&idempotency_key, record, body_hash, ttl);
    }

    // Unlock only after caching so concurrent duplicates get a 409 rather than
    // racing to acquire the lock before the entry is stored.
    store.unlock(&idempotency_key);

    if let Some(m) = &metrics {
        m.record_idempotency_miss();
    }

    // Reconstruct from original parts so all headers (including set-cookie) and
    // response extensions are preserved for outer middleware.
    Ok(Response::from_parts(resp_parts, Body::from(resp_bytes)))
}

fn handle_cache_hit(
    entry: IdempotencyEntry,
    body_hash: &[u8],
    idempotency_key: &str,
    metrics: Option<&crate::middleware::MetricsCollector>,
) -> Response<Body> {
    if entry.body_hash != body_hash {
        tracing::debug!(
            idempotency.key = %idempotency_key,
            "Idempotency payload mismatch — returning 422"
        );
        return Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .body(Body::from("idempotency key reused with different payload"))
            .unwrap();
    }

    tracing::debug!(
        idempotency.key = %idempotency_key,
        idempotency.replayed = true,
        "Idempotency cache hit — replaying stored response"
    );
    if let Some(m) = metrics {
        m.record_idempotency_hit();
    }

    let mut builder = Response::builder().status(entry.record.status);
    for (name, value) in &entry.record.headers {
        builder = builder.header(name.as_str(), value.as_slice());
    }
    builder
        .header(X_IDEMPOTENT_REPLAYED, "true")
        .body(Body::from(entry.record.body))
        .unwrap()
}
