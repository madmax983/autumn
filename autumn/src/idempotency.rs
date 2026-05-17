use bytes::Bytes;
use futures::StreamExt as FuturesStreamExt;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, Response, StatusCode, request::Parts};
use sha2::Digest as _;
use tower::{Layer, Service};

static IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
static X_IDEMPOTENT_REPLAYED: &str = "x-idempotent-replayed";

/// Maximum response body size stored in the idempotency cache. Responses
/// larger than this are returned to the client as-is but not cached, so a
/// subsequent retry with the same key will re-execute the handler.
const MAX_CACHEABLE_RESPONSE_BODY: usize = 10 * 1024 * 1024; // 10 MiB

/// Fallback request body read limit used when the upload middleware extension
/// is absent (e.g. when `IdempotencyLayer` is used directly without the full
/// framework stack). Matches the framework default for
/// `security.upload.max_request_size_bytes`.
const DEFAULT_REQUEST_BODY_LIMIT: usize = 32 * 1024 * 1024; // 32 MiB

const fn is_mutating_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn compute_body_hash(bytes: &[u8], content_type: Option<&[u8]>) -> Vec<u8> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"content-type:");
    if let Some(content_type) = content_type {
        hasher.update(content_type);
    }
    hasher.update(b"\nbody:");
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().fold(
        String::with_capacity(bytes.as_ref().len() * 2),
        |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        },
    )
}

fn principal_scope_digest(auth: Option<&str>, session_id: Option<&str>) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"authorization:");
    if let Some(auth) = auth {
        hasher.update(auth.as_bytes());
    }
    hasher.update(b"\nsession:");
    if let Some(session_id) = session_id {
        hasher.update(session_id.as_bytes());
    }
    hex_lower(hasher.finalize())
}

/// Namespace the cache key by method, path, a stable principal digest, and the
/// client-supplied idempotency key.
///
/// Namespacing by method+path prevents cross-endpoint cache collisions (P2).
/// Namespacing by Authorization and session scope prevents cross-principal
/// collisions (P1), including cookie-backed authenticated sessions.
fn build_storage_key(
    idempotency_key: &str,
    method: &Method,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    session_id: Option<&str>,
) -> String {
    let path = uri
        .path_and_query()
        .map_or_else(|| uri.path(), |pq| pq.as_str());
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .filter(|auth| !auth.is_empty());
    let principal = principal_scope_digest(auth, session_id);
    format!("{method}:{path}:{principal}:{idempotency_key}")
}

async fn build_storage_key_for_parts(
    idempotency_key: &str,
    parts: &axum::http::request::Parts,
) -> String {
    let method = parts.method.clone();
    let uri = parts.uri.clone();
    let headers = parts.headers.clone();
    let session = parts.extensions.get::<crate::session::Session>().cloned();
    let session_id = if let Some(session) = session
        && session.is_cookie_backed().await
    {
        Some(session.id().await)
    } else {
        None
    };
    build_storage_key(
        idempotency_key,
        &method,
        &uri,
        &headers,
        session_id.as_deref(),
    )
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
    fn try_lock(&self, key: &str, lock_ttl: Duration) -> bool;

    /// Release the in-flight lock for `key`.
    fn unlock(&self, key: &str);

    /// The preferred TTL for this store. Used by [`IdempotencyLayer::new`] as
    /// the default expiry when no explicit `.with_ttl()` is given. Defaults to
    /// 24 hours if the store does not override this method.
    fn default_ttl(&self) -> Duration {
        Duration::from_secs(86_400)
    }
}

// ── Memory store ──────────────────────────────────────────────────────────────

/// In-memory idempotency store backed by a `RwLock<HashMap>`.
///
/// Evicts expired entries lazily on `get`. In-flight markers remain held until
/// `unlock` because this process owns both the handler and the in-memory lock.
///
/// Suitable for single-process deployments and integration tests. For
/// multi-replica deployments configure `backend = "redis"` in `autumn.toml`.
pub struct MemoryIdempotencyStore {
    entries: RwLock<HashMap<String, IdempotencyEntry>>,
    in_flight: RwLock<HashSet<String>>,
    /// Counts `set` calls to trigger periodic expired-entry eviction.
    write_count: AtomicU64,
    default_ttl: Duration,
}

impl MemoryIdempotencyStore {
    #[must_use]
    pub fn new(default_ttl: Duration) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            in_flight: RwLock::new(HashSet::new()),
            write_count: AtomicU64::new(0),
            default_ttl,
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

    fn try_lock(&self, key: &str, _lock_ttl: Duration) -> bool {
        let mut in_flight = self.in_flight.write().unwrap();
        // Check only the requested key's active in-flight marker.
        if in_flight.contains(key) {
            return false; // still in flight
        }
        // Not locked: acquire until the handler finishes and unlocks.
        in_flight.insert(key.to_owned());
        true
    }

    fn unlock(&self, key: &str) {
        self.in_flight.write().unwrap().remove(key);
    }

    fn default_ttl(&self) -> Duration {
        self.default_ttl
    }
}

// ── Redis store ───────────────────────────────────────────────────────────────

#[cfg(feature = "redis")]
mod redis_store {
    use super::{IdempotencyEntry, IdempotencyRecord, IdempotencyStore};
    use redis::{AsyncCommands, Client, aio::ConnectionManager, aio::ConnectionManagerConfig};
    use serde::{Deserialize, Serialize};
    use std::time::{Duration, Instant};

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
                    let data: Option<Vec<u8>> = match conn.get(&redis_key).await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Redis GET failed for idempotency key; \
                                 treating as cache miss (idempotency degraded)"
                            );
                            None
                        }
                    };
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

        fn try_lock(&self, key: &str, lock_ttl: Duration) -> bool {
            let lock_key = self.lock_key(key);
            let lock_ttl_secs = lock_ttl.as_secs().max(1);
            let mut conn = self.connection.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    let result: Result<Option<String>, _> = redis::cmd("SET")
                        .arg(&lock_key)
                        .arg("1")
                        .arg("NX")
                        .arg("EX")
                        .arg(lock_ttl_secs)
                        .query_async(&mut conn)
                        .await;
                    match result {
                        Ok(opt) => opt.is_some(), // Some("OK") = acquired, None = already held
                        Err(e) => {
                            // Redis unavailable: fail closed so concurrent retries during an
                            // outage cannot both enter the handler and duplicate side effects.
                            // Clients receive 409 and should retry; once Redis recovers the
                            // lock can be acquired normally.
                            tracing::warn!(
                                error = %e,
                                "Redis idempotency lock unavailable; \
                                 failing closed to prevent duplicate processing"
                            );
                            false
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

#[derive(Clone)]
struct IdempotencyReplayResponse {
    record: IdempotencyRecord,
}

impl IdempotencyReplayResponse {
    fn into_response(self) -> Response<Body> {
        response_from_record(self.record)
    }
}

/// Inner route layer used by Autumn-generated handlers to stop before the
/// mutating handler when an outer idempotency layer has already found a replay.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdempotencyReplayLayer;

#[derive(Clone)]
pub struct IdempotencyReplayService<S> {
    inner: S,
}

impl<S> Layer<S> for IdempotencyReplayLayer {
    type Service = IdempotencyReplayService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        IdempotencyReplayService { inner }
    }
}

impl<S> Service<Request<Body>> for IdempotencyReplayService<S>
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

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        if let Some(replay) = req.extensions_mut().remove::<IdempotencyReplayResponse>() {
            return Box::pin(async move { Ok(replay.into_response()) });
        }

        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move { inner.call(req).await })
    }
}

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
    in_flight_ttl: Duration,
    replay_through_inner: bool,
    metrics: Option<crate::middleware::MetricsCollector>,
}

impl IdempotencyLayer {
    #[must_use]
    pub fn new(store: Arc<dyn IdempotencyStore>) -> Self {
        let ttl = store.default_ttl();
        Self {
            store,
            ttl,
            in_flight_ttl: ttl,
            replay_through_inner: false,
            metrics: None,
        }
    }

    #[must_use]
    pub const fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    #[must_use]
    pub const fn with_in_flight_ttl(mut self, ttl: Duration) -> Self {
        self.in_flight_ttl = ttl;
        self
    }

    #[must_use]
    pub const fn replay_through_inner(mut self) -> Self {
        self.replay_through_inner = true;
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
            in_flight_ttl: self.in_flight_ttl,
            replay_through_inner: self.replay_through_inner,
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
    in_flight_ttl: Duration,
    replay_through_inner: bool,
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
        // Clone-and-swap: the instance that was polled ready becomes `inner`
        // for this call; `self.inner` is replaced with a fresh clone for the
        // next call. This preserves Tower backpressure semantics.
        let clone = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, clone);
        let store = self.store.clone();
        let ttl = self.ttl;
        let in_flight_ttl = self.in_flight_ttl;
        let replay_through_inner = self.replay_through_inner;
        let metrics = self.metrics.clone();
        Box::pin(handle_idempotent_request(
            inner,
            store,
            ttl,
            in_flight_ttl,
            replay_through_inner,
            metrics,
            req,
        ))
    }
}

fn request_body_too_large_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .body(Body::from(
            "request body too large for idempotency middleware",
        ))
        .unwrap()
}

fn in_flight_conflict_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::CONFLICT)
        .header("retry-after", "1")
        .body(Body::from(
            "a request with this idempotency key is already being processed; \
             retry after 1 second",
        ))
        .unwrap()
}

struct PreparedIdempotencyRequest {
    idempotency_key: String,
    storage_key: String,
    body_hash: Vec<u8>,
    parts: Parts,
    body_bytes: Bytes,
}

fn request_idempotency_key(req: &Request<Body>) -> Option<String> {
    let key = req
        .headers()
        .get(IDEMPOTENCY_KEY_HEADER)?
        .to_str()
        .unwrap_or("");
    (!key.is_empty()).then(|| key.to_owned())
}

async fn prepare_idempotency_request(
    idempotency_key: String,
    req: Request<Body>,
) -> Result<PreparedIdempotencyRequest, Response<Body>> {
    let (parts, body) = req.into_parts();
    let storage_key = build_storage_key_for_parts(&idempotency_key, &parts).await;
    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .map(axum::http::HeaderValue::as_bytes);
    let body_limit = parts
        .extensions
        .get::<crate::security::config::UploadConfig>()
        .map_or(DEFAULT_REQUEST_BODY_LIMIT, |c| c.max_request_size_bytes);
    let body_bytes = axum::body::to_bytes(body, body_limit)
        .await
        .map_err(|_| request_body_too_large_response())?;
    let body_hash = compute_body_hash(&body_bytes, content_type);

    Ok(PreparedIdempotencyRequest {
        idempotency_key,
        storage_key,
        body_hash,
        parts,
        body_bytes,
    })
}

async fn handle_idempotent_request<S>(
    mut inner: S,
    store: Arc<dyn IdempotencyStore>,
    ttl: Duration,
    in_flight_ttl: Duration,
    replay_through_inner: bool,
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

    let Some(idempotency_key) = request_idempotency_key(&req) else {
        return inner.call(req).await;
    };

    let prepared = match prepare_idempotency_request(idempotency_key, req).await {
        Ok(prepared) => prepared,
        Err(response) => return Ok(response),
    };

    // ── Cache hit ──────────────────────────────────────────────────────────
    if let Some(entry) = store.get(&prepared.storage_key) {
        return replay_cache_hit(
            &mut inner,
            entry,
            prepared,
            metrics.as_ref(),
            replay_through_inner,
        )
        .await;
    }

    // ── In-flight check (concurrent duplicate) ─────────────────────────────
    if !store.try_lock(&prepared.storage_key, in_flight_ttl) {
        tracing::debug!(
            idempotency.key = %prepared.idempotency_key,
            "Idempotency key already in flight — returning 409"
        );
        metrics
            .as_ref()
            .inspect(|m| m.record_idempotency_conflict());
        return Ok(in_flight_conflict_response());
    }

    // Double-check after acquiring the lock: a concurrent request may have
    // completed between our miss check and lock acquisition.
    if let Some(entry) = store.get(&prepared.storage_key) {
        store.unlock(&prepared.storage_key);
        return replay_cache_hit(
            &mut inner,
            entry,
            prepared,
            metrics.as_ref(),
            replay_through_inner,
        )
        .await;
    }

    handle_cache_miss(inner, store.as_ref(), ttl, prepared, metrics.as_ref()).await
}

async fn handle_cache_miss<S>(
    mut inner: S,
    store: &dyn IdempotencyStore,
    ttl: Duration,
    prepared: PreparedIdempotencyRequest,
    metrics: Option<&crate::middleware::MetricsCollector>,
) -> Result<Response<Body>, std::convert::Infallible>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = std::convert::Infallible>
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    let PreparedIdempotencyRequest {
        idempotency_key,
        storage_key,
        body_hash,
        parts,
        body_bytes,
    } = prepared;

    tracing::debug!(
        idempotency.key = %idempotency_key,
        "Idempotency cache miss — forwarding to handler"
    );

    let response = inner
        .call(Request::from_parts(parts, Body::from(body_bytes)))
        .await?;
    let (resp_parts, resp_body) = response.into_parts();

    // Collect up to the cache cap; stream oversized bodies through without
    // buffering to avoid materialising large responses in memory.
    let resp_bytes = match collect_response_for_cache(resp_body).await {
        CollectedResponseBody::StreamError(passthrough_body) => {
            store.unlock(&storage_key);
            tracing::warn!(
                idempotency.key = %idempotency_key,
                "I/O error reading response body; passing the body error through without storing idempotency entry"
            );
            return Ok(Response::from_parts(resp_parts, passthrough_body));
        }
        CollectedResponseBody::TooLarge {
            passthrough_body, ..
        } => {
            // Body exceeded MAX_CACHEABLE_RESPONSE_BODY — stream through.
            store.unlock(&storage_key);
            tracing::debug!(
                idempotency.key = %idempotency_key,
                limit_bytes = MAX_CACHEABLE_RESPONSE_BODY,
                "Response body exceeded cache limit; streaming through without caching"
            );
            return Ok(Response::from_parts(resp_parts, passthrough_body));
        }
        CollectedResponseBody::Cacheable(bytes) => bytes,
    };

    // Cache successful 2xx/3xx responses; store before unlocking so concurrent duplicates
    // still see a locked key rather than racing to re-execute the handler.
    let status = resp_parts.status.as_u16();
    if (200u32..400).contains(&u32::from(status)) {
        let record = IdempotencyRecord {
            status,
            // set-cookie excluded: prevents session fixation on replay.
            headers: extract_replay_headers(&resp_parts.headers),
            body: resp_bytes.to_vec(),
        };
        store.set(&storage_key, record, body_hash, ttl);
    }
    store.unlock(&storage_key);

    if let Some(m) = metrics {
        m.record_idempotency_miss();
    }

    // Reconstruct from original parts — preserves set-cookie and extensions.
    Ok(Response::from_parts(resp_parts, Body::from(resp_bytes)))
}

/// Collect response body bytes up to `MAX_CACHEABLE_RESPONSE_BODY`.
///
/// Returns:
/// - `Cacheable(bytes)` — body is within the limit and fully collected
/// - `TooLarge(body)` — body exceeded the limit; the returned `Body` chains the
///   already-read bytes with the remaining stream for pass-through delivery
/// - `StreamError(body)` — reading the body stream failed; the returned `Body`
///   preserves the already-read bytes and then yields the original stream error
enum CollectedResponseBody {
    Cacheable(Bytes),
    TooLarge {
        passthrough_body: Body,
        #[cfg_attr(not(test), allow(dead_code))]
        buffered_len: usize,
    },
    StreamError(Body),
}

async fn collect_response_for_cache(body: Body) -> CollectedResponseBody {
    collect_response_for_cache_with_limit(body, MAX_CACHEABLE_RESPONSE_BODY).await
}

async fn collect_response_for_cache_with_limit(body: Body, limit: usize) -> CollectedResponseBody {
    let mut buf = Vec::<u8>::new();
    let mut data_stream = body.into_data_stream();
    loop {
        match data_stream.next().await {
            None => break,
            Some(Err(err)) => {
                let leading = Bytes::from(buf);
                let passthrough =
                    Body::from_stream(futures::stream::iter(vec![Ok(leading), Err(err)]));
                return CollectedResponseBody::StreamError(passthrough);
            }
            Some(Ok(chunk)) => {
                if chunk.len() > limit.saturating_sub(buf.len()) {
                    let buffered_len = buf.len();
                    let mut leading_chunks = Vec::with_capacity(2);
                    if !buf.is_empty() {
                        leading_chunks.push(Ok::<Bytes, axum::Error>(Bytes::from(buf)));
                    }
                    leading_chunks.push(Ok::<Bytes, axum::Error>(chunk));
                    let passthrough =
                        Body::from_stream(futures::stream::iter(leading_chunks).chain(data_stream));
                    return CollectedResponseBody::TooLarge {
                        passthrough_body: passthrough,
                        buffered_len,
                    };
                }
                buf.extend_from_slice(&chunk);
            }
        }
    }
    CollectedResponseBody::Cacheable(Bytes::from(buf))
}

async fn replay_cache_hit<S>(
    inner: &mut S,
    entry: IdempotencyEntry,
    prepared: PreparedIdempotencyRequest,
    metrics: Option<&crate::middleware::MetricsCollector>,
    replay_through_inner: bool,
) -> Result<Response<Body>, std::convert::Infallible>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = std::convert::Infallible>
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    let PreparedIdempotencyRequest {
        idempotency_key,
        body_hash,
        mut parts,
        body_bytes,
        ..
    } = prepared;

    if entry.body_hash != body_hash {
        tracing::debug!(
            idempotency.key = %idempotency_key,
            "Idempotency payload mismatch — returning 422"
        );
        return Ok(Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .body(Body::from("idempotency key reused with different payload"))
            .unwrap());
    }

    tracing::debug!(
        idempotency.key = %idempotency_key,
        idempotency.replayed = true,
        "Idempotency cache hit — replaying stored response"
    );
    if let Some(m) = metrics {
        m.record_idempotency_hit();
    }

    let replay = IdempotencyReplayResponse {
        record: entry.record,
    };
    if replay_through_inner {
        parts.extensions.insert(replay);
        return inner
            .call(Request::from_parts(parts, Body::from(body_bytes)))
            .await;
    }

    Ok(replay.into_response())
}

fn response_from_record(record: IdempotencyRecord) -> Response<Body> {
    let mut builder = Response::builder().status(record.status);
    for (name, value) in &record.headers {
        builder = builder.header(name.as_str(), value.as_slice());
    }
    builder
        .header(X_IDEMPOTENT_REPLAYED, "true")
        .body(Body::from(record.body))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::AUTHORIZATION;
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::sync::Mutex;
    use tower::ServiceExt;

    #[derive(Clone, Default)]
    struct RecordingStore {
        keys: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingStore {
        fn keys(&self) -> Vec<String> {
            self.keys
                .lock()
                .expect("recording store lock poisoned")
                .clone()
        }

        fn record_key(&self, key: &str) {
            self.keys
                .lock()
                .expect("recording store lock poisoned")
                .push(key.to_owned());
        }
    }

    impl IdempotencyStore for RecordingStore {
        fn get(&self, key: &str) -> Option<IdempotencyEntry> {
            self.record_key(key);
            None
        }

        fn set(&self, key: &str, _record: IdempotencyRecord, _body_hash: Vec<u8>, _ttl: Duration) {
            self.record_key(key);
        }

        fn try_lock(&self, key: &str, _lock_ttl: Duration) -> bool {
            self.record_key(key);
            true
        }

        fn unlock(&self, key: &str) {
            self.record_key(key);
        }
    }

    fn idempotent_post(path: &str, key: &str, body: &'static str) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .body(Body::from(body))
            .expect("request builder should be valid")
    }

    fn session_with_user(session_id: &str, user_id: &str) -> crate::session::Session {
        let mut data = HashMap::new();
        data.insert("user_id".to_owned(), user_id.to_owned());
        crate::session::Session::new_for_test(session_id.to_owned(), data)
    }

    fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().fold(
            String::with_capacity(bytes.as_ref().len() * 2),
            |mut out, byte| {
                use std::fmt::Write as _;
                let _ = write!(out, "{byte:02x}");
                out
            },
        )
    }

    fn expected_principal_digest(auth: Option<&str>, session_id: Option<&str>) -> String {
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"authorization:");
        if let Some(auth) = auth {
            hasher.update(auth.as_bytes());
        }
        hasher.update(b"\nsession:");
        if let Some(session_id) = session_id {
            hasher.update(session_id.as_bytes());
        }
        hex_lower(hasher.finalize())
    }

    #[tokio::test]
    async fn response_body_stream_errors_are_not_replaced_with_empty_success() {
        let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));
        let service = IdempotencyLayer::new(store).layer(tower::service_fn(
            |_req: Request<Body>| async move {
                let stream = futures::stream::iter(vec![
                    Ok::<Bytes, std::io::Error>(Bytes::from_static(b"partial")),
                    Err(std::io::Error::other("stream failed")),
                ]);
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from_stream(stream))
                        .expect("response builder should be valid"),
                )
            },
        ));

        let response = service
            .oneshot(idempotent_post("/stream", "stream-key", "same"))
            .await
            .expect("request should complete");

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await;
        assert!(
            body.is_err(),
            "idempotency middleware must preserve response body stream errors"
        );
    }

    #[tokio::test]
    async fn collect_response_checks_chunk_size_before_buffering_past_cap() {
        let chunk = Bytes::from(vec![b'x'; 64]);
        match collect_response_for_cache_with_limit(Body::from(chunk), 10).await {
            CollectedResponseBody::TooLarge {
                passthrough_body,
                buffered_len,
            } => {
                assert_eq!(
                    buffered_len, 0,
                    "single over-cap chunk must not be copied into the cache buffer first"
                );
                let delivered = axum::body::to_bytes(passthrough_body, usize::MAX)
                    .await
                    .expect("passthrough body should collect");
                assert_eq!(delivered.len(), 64);
            }
            CollectedResponseBody::Cacheable(_) => {
                panic!("over-cap chunk must not be considered cacheable")
            }
            CollectedResponseBody::StreamError(_) => panic!("body should not stream-error"),
        }
    }

    #[tokio::test]
    async fn cookie_session_extension_scopes_idempotency_storage_key() {
        let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));
        let mut service = IdempotencyLayer::new(store).layer(tower::service_fn(
            |req: Request<Body>| async move {
                let session = req
                    .extensions()
                    .get::<crate::session::Session>()
                    .cloned()
                    .expect("session extension should be present");
                let user_id = session
                    .get("user_id")
                    .await
                    .expect("session should contain user_id");
                Ok::<_, Infallible>(Response::new(Body::from(user_id)))
            },
        ));

        let mut alice_req = idempotent_post("/orders", "shared-key", "same");
        alice_req
            .extensions_mut()
            .insert(session_with_user("session-alice", "alice"));
        let alice_response = service
            .ready()
            .await
            .expect("service should be ready")
            .call(alice_req)
            .await
            .expect("alice request should complete");
        let alice_body = axum::body::to_bytes(alice_response.into_body(), usize::MAX)
            .await
            .expect("alice body should collect");
        assert_eq!(alice_body, Bytes::from_static(b"alice"));

        let mut bob_req = idempotent_post("/orders", "shared-key", "same");
        bob_req
            .extensions_mut()
            .insert(session_with_user("session-bob", "bob"));
        let bob_response = service
            .ready()
            .await
            .expect("service should be ready")
            .call(bob_req)
            .await
            .expect("bob request should complete");
        assert!(
            bob_response.headers().get(X_IDEMPOTENT_REPLAYED).is_none(),
            "a different cookie-backed session must not replay another user's response"
        );
        let bob_body = axum::body::to_bytes(bob_response.into_body(), usize::MAX)
            .await
            .expect("bob body should collect");
        assert_eq!(bob_body, Bytes::from_static(b"bob"));
    }

    #[tokio::test]
    async fn storage_key_hashes_authorization_with_stable_sha256_digest() {
        let observed_store = RecordingStore::default();
        let service = IdempotencyLayer::new(Arc::new(observed_store.clone())).layer(
            tower::service_fn(|_req: Request<Body>| async {
                Ok::<_, Infallible>(Response::new(Body::from("ok")))
            }),
        );
        let request = Request::builder()
            .method(Method::POST)
            .uri("/payments")
            .header(IDEMPOTENCY_KEY_HEADER, "pay-once")
            .header(AUTHORIZATION, "Bearer stable-token")
            .body(Body::from("same"))
            .expect("request builder should be valid");

        let response = service
            .oneshot(request)
            .await
            .expect("request should complete");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should collect");
        assert_eq!(body, Bytes::from_static(b"ok"));

        let keys = observed_store.keys();
        let storage_key = keys.first().expect("storage key should be recorded");
        let segments = storage_key.splitn(4, ':').collect::<Vec<_>>();
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0], "POST");
        assert_eq!(segments[1], "/payments");
        assert_eq!(
            segments[2],
            expected_principal_digest(Some("Bearer stable-token"), None)
        );
        assert_eq!(segments[3], "pay-once");
    }
}
