use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode};
use tower::{Layer, Service};

static IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
static X_IDEMPOTENT_REPLAYED: &str = "x-idempotent-replayed";

fn is_mutating_method(method: &Method) -> bool {
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

fn extract_replay_headers(response: &Response<Body>) -> Vec<(String, Vec<u8>)> {
    let skip = [
        "connection",
        "transfer-encoding",
        "keep-alive",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "x-idempotent-replayed",
    ];
    response
        .headers()
        .iter()
        .filter(|(name, _)| !skip.contains(&name.as_str()))
        .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
        .collect()
}

/// Stored response for an idempotency key.
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

/// Pluggable storage backend for idempotency entries.
pub trait IdempotencyStore: Send + Sync + 'static {
    fn get(&self, key: &str) -> Option<IdempotencyEntry>;
    fn set(&self, key: &str, record: IdempotencyRecord, body_hash: Vec<u8>, ttl: Duration);
}

/// In-memory idempotency store backed by a `RwLock<HashMap>`.
///
/// Evicts expired entries lazily on `get` and proactively on `set`.
/// Suitable for single-process deployments and tests.
pub struct MemoryIdempotencyStore {
    entries: RwLock<HashMap<String, IdempotencyEntry>>,
}

impl MemoryIdempotencyStore {
    pub fn new(_default_ttl: Duration) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }
}

impl IdempotencyStore for MemoryIdempotencyStore {
    fn get(&self, key: &str) -> Option<IdempotencyEntry> {
        let entries = self.entries.read().unwrap();
        let entry = entries.get(key)?;
        if entry.expires_at > Instant::now() {
            Some(entry.clone())
        } else {
            None
        }
    }

    fn set(&self, key: &str, record: IdempotencyRecord, body_hash: Vec<u8>, ttl: Duration) {
        let expires_at = Instant::now() + ttl;
        let entry = IdempotencyEntry {
            record,
            body_hash,
            expires_at,
        };
        let mut entries = self.entries.write().unwrap();
        let now = Instant::now();
        entries.retain(|_, v| v.expires_at > now);
        entries.insert(key.to_owned(), entry);
    }
}

/// Tower [`Layer`] that enforces HTTP idempotency semantics.
///
/// Applies only to mutating HTTP methods (POST, PUT, PATCH, DELETE).
/// Requests without an `Idempotency-Key` header are passed through unchanged.
///
/// On a cache hit with a matching body hash the stored response is replayed
/// with an `X-Idempotent-Replayed: true` header.  On a hash mismatch a
/// `422 Unprocessable Entity` is returned immediately.
#[derive(Clone)]
pub struct IdempotencyLayer {
    store: Arc<dyn IdempotencyStore>,
    ttl: Duration,
}

impl IdempotencyLayer {
    pub fn new(store: Arc<dyn IdempotencyStore>) -> Self {
        Self {
            store,
            ttl: Duration::from_secs(86_400),
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
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
        }
    }
}

/// Tower [`Service`] produced by [`IdempotencyLayer`].
#[derive(Clone)]
pub struct IdempotencyService<S> {
    inner: S,
    store: Arc<dyn IdempotencyStore>,
    ttl: Duration,
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
        let mut inner = self.inner.clone();
        let store = self.store.clone();
        let ttl = self.ttl;

        Box::pin(async move {
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
            let body_bytes = axum::body::to_bytes(body, 10 * 1024 * 1024)
                .await
                .unwrap_or_default();
            let body_hash = compute_body_hash(&body_bytes);

            if let Some(entry) = store.get(&idempotency_key) {
                if entry.body_hash != body_hash {
                    let response = Response::builder()
                        .status(StatusCode::UNPROCESSABLE_ENTITY)
                        .body(Body::from("idempotency key reused with different payload"))
                        .unwrap();
                    return Ok(response);
                }

                let mut builder = Response::builder().status(entry.record.status);
                for (name, value) in &entry.record.headers {
                    builder = builder.header(name.as_str(), value.as_slice());
                }
                builder = builder.header(X_IDEMPOTENT_REPLAYED, "true");
                let response = builder
                    .body(Body::from(entry.record.body.clone()))
                    .unwrap();
                return Ok(response);
            }

            let req = Request::from_parts(parts, Body::from(body_bytes));
            let response = inner.call(req).await?;

            let status = response.status().as_u16();
            let headers = extract_replay_headers(&response);
            let resp_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_default();

            let record = IdempotencyRecord {
                status,
                headers: headers.clone(),
                body: resp_bytes.to_vec(),
            };
            store.set(&idempotency_key, record, body_hash, ttl);

            let mut builder = Response::builder().status(status);
            for (name, value) in &headers {
                builder = builder.header(name.as_str(), value.as_slice());
            }
            let fresh = builder.body(Body::from(resp_bytes.to_vec())).unwrap();
            Ok(fresh)
        })
    }
}
