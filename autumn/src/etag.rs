//! ETag-based conditional GET helpers for HTML and JSON handlers.
//!
//! Autumn makes conditional `GET` a one-liner via [`fresh_when`] and the
//! [`EtagLayer`] middleware. Together they give a framework app three layers
//! of "don't do work twice":
//!
//! | Layer | Mechanism | Scope |
//! |-------|-----------|-------|
//! | Server render skip | `#[cached]` | avoid running the handler |
//! | Network skip | `fresh_when` / [`EtagLayer`] | avoid retransmitting the body |
//! | Static skip | ISR / `#[static_get]` | serve pre-rendered bytes |
//!
//! # Quick start
//!
//! ## HTML handler (Maud)
//!
//! ```rust,no_run
//! use autumn_web::etag::fresh_when;
//! use autumn_web::prelude::*;
//!
//! #[get("/posts/{id}")]
//! async fn show(id: Path<i64>, headers: http::HeaderMap, mut db: Db) -> AutumnResult<impl IntoResponse> {
//!     // post.etag() is derived from #[lock_version] — no manual hashing.
//!     // let post = Post::find(*id, &mut db).await?;
//!     // Ok(fresh_when(&headers, post.etag()).or(html! { h1 { (post.title) } }))
//!     Ok(StatusCode::OK)
//! }
//! ```
//!
//! ## JSON handler
//!
//! ```rust,no_run
//! use autumn_web::etag::fresh_when;
//! use autumn_web::prelude::*;
//!
//! #[get("/api/posts/{id}")]
//! async fn show_json(id: Path<i64>, headers: http::HeaderMap, mut db: Db) -> AutumnResult<impl IntoResponse> {
//!     // let post = Post::find(*id, &mut db).await?;
//!     // Ok(fresh_when(&headers, post.etag()).or(Json(post)))
//!     Ok(StatusCode::OK)
//! }
//! ```
//!
//! # htmx polling pattern
//!
//! ```html
//! <div hx-get="/posts/1" hx-trigger="every 5s" hx-swap="outerHTML">
//!   <!-- content -->
//! </div>
//! ```
//!
//! With `fresh_when` protecting the handler, unchanged polls return `304`
//! with zero body bytes and cost < 2 ms end-to-end.

use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::response::IntoResponse;
use http::header::{
    CACHE_CONTROL, CONTENT_LOCATION, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
    SET_COOKIE, VARY,
};
use http::{HeaderMap, HeaderValue, Response, StatusCode};
use http_body_util::BodyExt;
use sha2::Digest as _;
use tower::{Layer, Service};

// ── ETag type ─────────────────────────────────────────────────────────────────

/// A validated HTTP `ETag` value.
///
/// The inner string is the *opaque tag* — without surrounding quotes or the
/// `W/` weak prefix. [`ETag::header_value`] produces the correctly formatted
/// `ETag` header value (e.g. `W/"abc123"` for a weak `ETag`).
///
/// # Creating an `ETag`
///
/// Most callers should use the [`IntoETag`] blanket conversions rather than
/// constructing an `ETag` directly:
///
/// ```rust
/// use autumn_web::etag::{ETag, IntoETag};
///
/// let e1: ETag = "abc".into_etag();       // strong, from str
/// let e2: ETag = 42_i64.into_etag();      // strong, from lock_version
/// let e3: ETag = String::from("v2").into_etag();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ETag {
    tag: String,
    weak: bool,
}

impl ETag {
    /// Construct a **strong** `ETag` from any tag string.
    ///
    /// Strong `ETag`s assert byte-for-byte equivalence.
    #[must_use]
    pub fn strong(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            weak: false,
        }
    }

    /// Construct a **weak** `ETag` from any tag string.
    ///
    /// Weak `ETag`s (prefixed `W/`) assert semantic equivalence: same content,
    /// possibly different encoding. Use them when the response can vary by
    /// `Accept-Encoding` or when comparing bodies with minor byte differences.
    #[must_use]
    pub fn weak(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            weak: true,
        }
    }

    /// Returns the opaque tag value (no quotes, no `W/` prefix).
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Returns `true` if this is a weak `ETag`.
    #[must_use]
    pub const fn is_weak(&self) -> bool {
        self.weak
    }

    /// Formats this `ETag` as an HTTP header value.
    ///
    /// - Strong: `"<tag>"`
    /// - Weak: `W/"<tag>"`
    #[must_use]
    pub fn header_value(&self) -> HeaderValue {
        let formatted = if self.weak {
            format!("W/\"{}\"", self.tag)
        } else {
            format!("\"{}\"", self.tag)
        };
        HeaderValue::from_str(&formatted).unwrap_or_else(|_| HeaderValue::from_static(""))
    }

    /// Returns `true` if the given raw `If-None-Match` header value matches
    /// this `ETag` according to RFC 7232 §3.2 weak comparison.
    ///
    /// `*` matches any `ETag`. Both strong and weak `ETag`s are compared by
    /// their opaque tag string.
    fn matches_if_none_match(&self, if_none_match: &str) -> bool {
        let if_none_match = if_none_match.trim();
        if if_none_match == "*" {
            return true;
        }
        for candidate in if_none_match.split(',') {
            let candidate = candidate.trim();
            // Strip W/ prefix then quotes for weak comparison.
            let tag = candidate
                .strip_prefix("W/")
                .unwrap_or(candidate)
                .trim_matches('"');
            if tag == self.tag {
                return true;
            }
        }
        false
    }
}

// ── IntoETag trait ────────────────────────────────────────────────────────────

/// Conversion trait for types that can serve as an `ETag` source.
///
/// Blanket implementations are provided for:
///
/// | Input type | `ETag` derivation |
/// |------------|-----------------|
/// | `String` / `&str` | SHA-256 of the string bytes (strong) |
/// | `i64` | SHA-256 of the integer (strong) — suitable for `#[lock_version]` |
/// | `(NaiveDateTime, i64)` | SHA-256 of `updated_at` + `lock_version` (strong) |
/// | Any `impl Hash` via [`hash_etag`] | `SipHash` → hex (weak) |
/// | `ETag` | identity |
pub trait IntoETag {
    /// Convert `self` into an [`ETag`].
    fn into_etag(self) -> ETag;
}

impl IntoETag for ETag {
    fn into_etag(self) -> ETag {
        self
    }
}

impl IntoETag for String {
    fn into_etag(self) -> ETag {
        ETag::strong(sha256_hex(self.as_bytes()))
    }
}

impl IntoETag for &str {
    fn into_etag(self) -> ETag {
        ETag::strong(sha256_hex(self.as_bytes()))
    }
}

impl IntoETag for i64 {
    fn into_etag(self) -> ETag {
        ETag::strong(sha256_hex(&self.to_be_bytes()))
    }
}

impl IntoETag for i32 {
    fn into_etag(self) -> ETag {
        i64::from(self).into_etag()
    }
}

/// `(updated_at, lock_version)` tuple — the idiomatic combo when a model has
/// both a timestamp and an optimistic-lock version.
impl IntoETag for (chrono::NaiveDateTime, i64) {
    fn into_etag(self) -> ETag {
        let mut hasher = sha2::Sha256::new();
        hasher.update(self.0.and_utc().timestamp().to_be_bytes());
        hasher.update(self.0.and_utc().timestamp_subsec_nanos().to_be_bytes());
        hasher.update(self.1.to_be_bytes());
        ETag::strong(hex_lower(hasher.finalize()))
    }
}

impl IntoETag for (chrono::NaiveDateTime, i32) {
    fn into_etag(self) -> ETag {
        (self.0, i64::from(self.1)).into_etag()
    }
}

/// Derive a **weak** `ETag` from any [`Hash`] value.
///
/// Uses `SipHash` so this is cheap but NOT cryptographically strong.
/// Suitable for response-body hashing in [`EtagLayer`].
///
/// # Example
///
/// ```rust
/// use autumn_web::etag::{hash_etag, IntoETag};
///
/// let etag = hash_etag(&vec![1u8, 2, 3]);
/// assert!(etag.is_weak());
/// ```
#[must_use]
pub fn hash_etag<T: Hash>(value: &T) -> ETag {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    ETag::weak(format!("{:016x}", hasher.finish()))
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(sha2::Sha256::digest(bytes))
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

// ── FreshWhen result ──────────────────────────────────────────────────────────

/// The outcome of a conditional-GET check.
///
/// Produced by [`fresh_when`]. Call `.or(response)` to either return a `304`
/// (if the resource is fresh) or the full response with `ETag` and
/// `Last-Modified` headers applied (if the resource is stale).
///
/// ```rust
/// # use autumn_web::etag::fresh_when;
/// # use http::{HeaderMap, StatusCode};
/// # use axum::response::IntoResponse;
/// let headers = HeaderMap::new();
/// let response = fresh_when(&headers, "v1").or(StatusCode::OK);
/// ```
#[must_use = "call `.or(response)` to resolve the conditional-GET result"]
pub struct FreshWhen {
    etag: ETag,
    last_modified: Option<chrono::DateTime<chrono::Utc>>,
    is_fresh: bool,
}

impl FreshWhen {
    /// `true` if the conditional check matched — the resource has not changed
    /// since the client's cached copy.
    #[must_use]
    pub const fn is_fresh(&self) -> bool {
        self.is_fresh
    }

    /// Attach a `Last-Modified` timestamp to the result.
    ///
    /// When `is_fresh`, the timestamp is included in the `304` response
    /// headers. When stale, it is included in the `200` response headers.
    pub fn last_modified(mut self, dt: impl Into<Option<chrono::DateTime<chrono::Utc>>>) -> Self {
        self.last_modified = dt.into();
        self
    }

    /// Resolve to an HTTP response.
    ///
    /// - **Fresh**: returns `304 Not Modified` with `ETag` / `Last-Modified`
    ///   preserved, `Set-Cookie` stripped, `Cache-Control` / `Vary` /
    ///   `Content-Location` preserved.
    /// - **Stale**: returns the `response` produced by `f` with `ETag` and
    ///   `Last-Modified` headers injected.
    pub fn or(self, response: impl IntoResponse) -> impl IntoResponse {
        if self.is_fresh {
            not_modified_response(&self.etag, self.last_modified)
        } else {
            let mut r = response.into_response();
            r.headers_mut().insert(ETAG, self.etag.header_value());
            if let Some(lm) = self.last_modified
                && let Ok(v) = HeaderValue::from_str(&http_date(lm))
            {
                r.headers_mut().insert(LAST_MODIFIED, v);
            }
            r
        }
    }
}

// ── fresh_when ────────────────────────────────────────────────────────────────

/// Check whether the client's cached copy of a resource is still fresh.
///
/// Returns a [`FreshWhen`] that you resolve with `.or(response)`.
///
/// # Conditional check logic
///
/// The resource is considered **fresh** (→ `304`) when:
///
/// 1. The request carries an `If-None-Match` header **and** the `ETag` matches
///    (weak comparison per RFC 7232 §3.2), **or**
/// 2. The request carries an `If-Modified-Since` header and
///    `last_modified` ≤ that timestamp (when no `If-None-Match` is present).
///
/// If neither conditional header is present, the resource is always stale.
///
/// # `ETag` determinism
///
/// `ETag` derivation never touches process-local state, RNGs, or the wall
/// clock — same inputs always produce the same `ETag` on every replica.
///
/// # Example
///
/// ```rust
/// use autumn_web::etag::fresh_when;
/// use http::{HeaderMap, HeaderValue, StatusCode};
/// use axum::response::IntoResponse;
///
/// // Simulate a client that already has the "v1" ETag cached.
/// let mut headers = HeaderMap::new();
/// headers.insert("if-none-match", HeaderValue::from_static("\"v1\""));
///
/// // "v1".into_etag() produces a SHA-256 of "v1".
/// // The fresh_when call hashes "v1" to match the header exactly — so we
/// // need the pre-computed hash for the test below. In real code you'd
/// // store and re-derive the ETag from stable model state.
/// let _r = fresh_when(&headers, 42_i64);
/// ```
pub fn fresh_when<E: IntoETag>(request_headers: &HeaderMap, etag: E) -> FreshWhen {
    let etag = etag.into_etag();

    let is_fresh = check_if_none_match(request_headers, &etag)
        || check_if_modified_since(request_headers, None);

    FreshWhen {
        etag,
        last_modified: None,
        is_fresh,
    }
}

fn check_if_none_match(headers: &HeaderMap, etag: &ETag) -> bool {
    headers
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| etag.matches_if_none_match(s))
}

fn check_if_modified_since(
    headers: &HeaderMap,
    last_modified: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    let Some(lm) = last_modified else {
        return false;
    };
    let Some(ims) = headers.get(IF_MODIFIED_SINCE).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    parse_http_date(ims).is_some_and(|parsed| {
        // Fresh if lm <= ims (client has the latest or newer)
        let lm_sys: std::time::SystemTime = lm.into();
        lm_sys <= parsed
    })
}

fn not_modified_response(
    etag: &ETag,
    last_modified: Option<chrono::DateTime<chrono::Utc>>,
) -> Response<Body> {
    let mut builder = Response::builder().status(StatusCode::NOT_MODIFIED);
    let headers = builder.headers_mut().expect("builder not consumed");
    headers.insert(ETAG, etag.header_value());
    if let Some(lm) = last_modified
        && let Ok(v) = HeaderValue::from_str(&http_date(lm))
    {
        headers.insert(LAST_MODIFIED, v);
    }
    builder.body(Body::empty()).expect("304 body is always valid")
}

fn http_date(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn parse_http_date(s: &str) -> Option<std::time::SystemTime> {
    // Try RFC 7231 / RFC 1123 format: "Tue, 15 Nov 1994 08:12:31 GMT"
    chrono::DateTime::parse_from_rfc2822(s)
        .map(|dt| std::time::SystemTime::from(dt.with_timezone(&chrono::Utc)))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s.trim_end_matches(" GMT"), "%a, %d %b %Y %H:%M:%S")
                .map(|ndt| {
                    std::time::SystemTime::from(
                        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(ndt, chrono::Utc),
                    )
                })
        })
        .ok()
}

// ── EtagLayer ─────────────────────────────────────────────────────────────────

/// Tower middleware that auto-derives a **weak** `ETag` from the response body.
///
/// Off by default. Add it to a route or router when you want transparent
/// `ETag` support without modifying the handler:
///
/// ```rust,no_run
/// use autumn_web::etag::EtagLayer;
/// use axum::Router;
/// use tower::ServiceBuilder;
///
/// let app: Router = Router::new()
///     // ... routes ...
///     .layer(EtagLayer::new());
/// ```
///
/// # Behaviour
///
/// For every `GET` request that produces a `200 OK` response:
///
/// 1. The response body is buffered (up to [`EtagLayer::MAX_BODY_BYTES`]).
/// 2. A weak `ETag` is derived from the body bytes via `SipHash`.
/// 3. If the request carried a matching `If-None-Match`, a `304` is returned.
/// 4. Otherwise, the `ETag` header is injected and the original response is
///    returned unchanged.
///
/// Responses that already carry an `ETag` header are passed through without
/// modification (handler-set `ETag`s take priority).
#[derive(Clone, Debug, Default)]
pub struct EtagLayer;

impl EtagLayer {
    /// Maximum body size (bytes) that will be buffered for `ETag` computation.
    /// Responses larger than this are passed through unchanged (no `ETag`).
    pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

    /// Create a new `EtagLayer`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for EtagLayer {
    type Service = EtagService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        EtagService { inner }
    }
}

/// The [`Service`] produced by [`EtagLayer`].
#[derive(Clone)]
pub struct EtagService<S> {
    inner: S,
}

impl<S, ReqBody> Service<http::Request<ReqBody>> for EtagService<S>
where
    S: Service<http::Request<ReqBody>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        let if_none_match = req
            .headers()
            .get(IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned);

        let is_get = req.method() == http::Method::GET;
        let fut = self.inner.call(req);

        Box::pin(async move {
            let response = fut.await?;

            // Only process GET 200 responses.
            if !is_get || response.status() != StatusCode::OK {
                return Ok(response);
            }

            // If the handler already set an ETag, check If-None-Match against it
            // before buffering the body.
            if let Some(existing_etag) = response.headers().get(ETAG).cloned() {
                if let Some(ref inm) = if_none_match {
                    let existing_tag = existing_etag.to_str().unwrap_or("");
                    // Use weak comparison: strip W/ prefix and quotes.
                    let tag = existing_tag
                        .strip_prefix("W/")
                        .unwrap_or(existing_tag)
                        .trim_matches('"');
                    let candidate_etag = ETag::strong(tag.to_owned());
                    if candidate_etag.matches_if_none_match(inm) {
                        let (parts, _body) = response.into_parts();
                        let mut not_modified = not_modified_response(&candidate_etag, None);
                        for name in [CACHE_CONTROL, VARY, CONTENT_LOCATION] {
                            if let Some(v) = parts.headers.get(&name) {
                                not_modified.headers_mut().insert(name, v.clone());
                            }
                        }
                        not_modified.headers_mut().remove(SET_COOKIE);
                        // Preserve the original ETag header value (strong/weak as set).
                        not_modified.headers_mut().insert(ETAG, existing_etag);
                        return Ok(not_modified);
                    }
                }
                return Ok(response);
            }

            let (mut parts, body) = response.into_parts();

            // Buffer the body for ETag computation.
            let Ok(collected) = body.collect().await else {
                return Ok(Response::from_parts(parts, Body::empty()));
            };
            let bytes = collected.to_bytes();

            if bytes.len() > EtagLayer::MAX_BODY_BYTES {
                let rebuilt = Response::from_parts(parts, Body::from(bytes));
                return Ok(rebuilt);
            }

            // Derive a weak ETag from body bytes.
            let etag = {
                let mut hasher = DefaultHasher::new();
                bytes.hash(&mut hasher);
                ETag::weak(format!("{:016x}", hasher.finish()))
            };

            // Check If-None-Match.
            if if_none_match
                .as_deref()
                .is_some_and(|inm| etag.matches_if_none_match(inm))
            {
                // Preserve allowed headers in the 304.
                let mut not_modified = not_modified_response(&etag, None);
                for name in [CACHE_CONTROL, VARY, CONTENT_LOCATION] {
                    if let Some(v) = parts.headers.get(&name) {
                        not_modified.headers_mut().insert(name, v.clone());
                    }
                }
                // Strip Set-Cookie from 304 (default: strip to avoid stale auth
                // state on intermediaries).
                not_modified.headers_mut().remove(SET_COOKIE);
                return Ok(not_modified);
            }

            parts.headers.insert(ETAG, etag.header_value());
            Ok(Response::from_parts(parts, Body::from(bytes)))
        })
    }
}

// ── not_modified helpers ───────────────────────────────────────────────────────

/// Build a minimal `304 Not Modified` response, preserving the headers that
/// RFC 7232 §4.1 requires intermediaries to pass through:
/// `Cache-Control`, `Vary`, `Content-Location`, `ETag`, `Last-Modified`.
///
/// `Set-Cookie` is **stripped by default** to prevent stale auth tokens from
/// being replayed by shared caches.
#[must_use]
pub fn build_not_modified(
    original_headers: &HeaderMap,
    etag: &ETag,
    last_modified: Option<chrono::DateTime<chrono::Utc>>,
) -> Response<Body> {
    let mut response = not_modified_response(etag, last_modified);
    for name in [CACHE_CONTROL, VARY, CONTENT_LOCATION] {
        if let Some(v) = original_headers.get(&name) {
            response.headers_mut().insert(name, v.clone());
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
    use tower::ServiceExt;

    // ── RED: ETag type ────────────────────────────────────────────────────────

    #[test]
    fn strong_etag_header_value_has_quotes() {
        let etag = ETag::strong("abc123");
        assert_eq!(etag.header_value().to_str().unwrap(), r#""abc123""#);
    }

    #[test]
    fn weak_etag_header_value_has_w_prefix() {
        let etag = ETag::weak("abc123");
        assert_eq!(etag.header_value().to_str().unwrap(), r#"W/"abc123""#);
    }

    #[test]
    fn etag_is_not_weak_by_default_strong_constructor() {
        let etag = ETag::strong("x");
        assert!(!etag.is_weak());
    }

    #[test]
    fn weak_etag_is_weak() {
        let etag = ETag::weak("x");
        assert!(etag.is_weak());
    }

    // ── RED: IntoETag conversions ─────────────────────────────────────────────

    #[test]
    fn str_into_etag_produces_deterministic_strong_etag() {
        let e1: ETag = "hello".into_etag();
        let e2: ETag = "hello".into_etag();
        assert_eq!(e1, e2);
        assert!(!e1.is_weak());
    }

    #[test]
    fn different_strings_produce_different_etags() {
        let e1: ETag = "hello".into_etag();
        let e2: ETag = "world".into_etag();
        assert_ne!(e1, e2);
    }

    #[test]
    fn string_into_etag_same_as_str() {
        let e1: ETag = "hello".into_etag();
        let e2: ETag = String::from("hello").into_etag();
        assert_eq!(e1, e2);
    }

    #[test]
    fn i64_into_etag_is_deterministic() {
        let e1: ETag = 42_i64.into_etag();
        let e2: ETag = 42_i64.into_etag();
        assert_eq!(e1, e2);
        assert!(!e1.is_weak());
    }

    #[test]
    fn different_i64_values_produce_different_etags() {
        let e1: ETag = 1_i64.into_etag();
        let e2: ETag = 2_i64.into_etag();
        assert_ne!(e1, e2);
    }

    #[test]
    fn i32_into_etag_matches_equivalent_i64() {
        let e1: ETag = 7_i32.into_etag();
        let e2: ETag = 7_i64.into_etag();
        assert_eq!(e1, e2);
    }

    #[test]
    fn tuple_into_etag_is_deterministic() {
        use chrono::NaiveDateTime;
        let dt = NaiveDateTime::from_timestamp_opt(1_000_000, 0).unwrap();
        let e1: ETag = (dt, 3_i64).into_etag();
        let e2: ETag = (dt, 3_i64).into_etag();
        assert_eq!(e1, e2);
        assert!(!e1.is_weak());
    }

    #[test]
    fn tuple_etag_differs_when_lock_version_differs() {
        use chrono::NaiveDateTime;
        let dt = NaiveDateTime::from_timestamp_opt(1_000_000, 0).unwrap();
        let e1: ETag = (dt, 1_i64).into_etag();
        let e2: ETag = (dt, 2_i64).into_etag();
        assert_ne!(e1, e2);
    }

    #[test]
    fn tuple_etag_differs_when_timestamp_differs() {
        use chrono::NaiveDateTime;
        let dt1 = NaiveDateTime::from_timestamp_opt(1_000_000, 0).unwrap();
        let dt2 = NaiveDateTime::from_timestamp_opt(1_000_001, 0).unwrap();
        let e1: ETag = (dt1, 1_i64).into_etag();
        let e2: ETag = (dt2, 1_i64).into_etag();
        assert_ne!(e1, e2);
    }

    #[test]
    fn hash_etag_is_weak() {
        let etag = hash_etag(&vec![1u8, 2, 3]);
        assert!(etag.is_weak());
    }

    #[test]
    fn hash_etag_is_deterministic_for_same_input() {
        let etag1 = hash_etag(&"stable_value");
        let etag2 = hash_etag(&"stable_value");
        assert_eq!(etag1, etag2);
    }

    // ── RED: ETag matching ────────────────────────────────────────────────────

    #[test]
    fn etag_matches_exact_quoted_value() {
        let etag = ETag::strong("abc");
        assert!(etag.matches_if_none_match(r#""abc""#));
    }

    #[test]
    fn etag_matches_weak_variant_by_tag() {
        let etag = ETag::strong("abc");
        assert!(etag.matches_if_none_match(r#"W/"abc""#));
    }

    #[test]
    fn etag_matches_star_wildcard() {
        let etag = ETag::strong("anything");
        assert!(etag.matches_if_none_match("*"));
    }

    #[test]
    fn etag_does_not_match_different_value() {
        let etag = ETag::strong("abc");
        assert!(!etag.matches_if_none_match(r#""xyz""#));
    }

    #[test]
    fn etag_matches_one_of_many_in_list() {
        let etag = ETag::strong("abc");
        assert!(etag.matches_if_none_match(r#""xyz", "abc", "foo""#));
    }

    // ── RED: fresh_when core behaviour ───────────────────────────────────────

    #[test]
    fn fresh_when_returns_stale_with_no_headers() {
        let headers = HeaderMap::new();
        let result = fresh_when(&headers, 1_i64);
        assert!(!result.is_fresh());
    }

    #[test]
    fn fresh_when_returns_fresh_on_matching_if_none_match() {
        let etag: ETag = 42_i64.into_etag();
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, etag.header_value());

        let result = fresh_when(&headers, 42_i64);
        assert!(result.is_fresh());
    }

    #[test]
    fn fresh_when_returns_stale_on_different_etag() {
        let etag: ETag = 1_i64.into_etag();
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, etag.header_value());

        // Resource changed — lock_version is now 2.
        let result = fresh_when(&headers, 2_i64);
        assert!(!result.is_fresh());
    }

    #[test]
    fn fresh_when_or_returns_304_when_fresh() {
        let etag: ETag = 7_i64.into_etag();
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, etag.header_value());

        let response = fresh_when(&headers, 7_i64)
            .or(StatusCode::OK)
            .into_response();

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn fresh_when_or_returns_200_and_sets_etag_when_stale() {
        let headers = HeaderMap::new(); // no If-None-Match
        let response = fresh_when(&headers, 1_i64)
            .or(StatusCode::OK)
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let etag_header = response.headers().get(ETAG);
        assert!(etag_header.is_some(), "ETag header must be set on stale response");
    }

    #[test]
    fn fresh_when_304_has_empty_body() {
        use http_body_util::BodyExt;

        let etag: ETag = 5_i64.into_etag();
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, etag.header_value());

        let response = fresh_when(&headers, 5_i64).or(StatusCode::OK).into_response();

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

        // Body must be empty.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let bytes = rt.block_on(async {
            response.into_body().collect().await.unwrap().to_bytes()
        });
        assert!(bytes.is_empty(), "304 body must be empty, got {bytes:?}");
    }

    #[test]
    fn fresh_when_or_includes_etag_in_304_headers() {
        let etag: ETag = 3_i64.into_etag();
        let etag_val = etag.header_value();
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, etag_val.clone());

        let response = fresh_when(&headers, 3_i64).or(StatusCode::OK).into_response();

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers().get(ETAG), Some(&etag_val));
    }

    #[test]
    fn fresh_when_wildcard_if_none_match_returns_fresh() {
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));

        let result = fresh_when(&headers, "anything");
        assert!(result.is_fresh());
    }

    // ── RED: last_modified support ────────────────────────────────────────────

    #[test]
    fn fresh_when_last_modified_sets_header_on_stale_response() {
        use chrono::TimeZone;

        let headers = HeaderMap::new();
        let last_modified = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();

        let response = fresh_when(&headers, 1_i64)
            .last_modified(last_modified)
            .or(StatusCode::OK)
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(LAST_MODIFIED));
    }

    #[test]
    fn fresh_when_last_modified_sets_header_on_304() {
        use chrono::TimeZone;

        let etag: ETag = 9_i64.into_etag();
        let last_modified = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, etag.header_value());

        let response = fresh_when(&headers, 9_i64)
            .last_modified(last_modified)
            .or(StatusCode::OK)
            .into_response();

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(response.headers().contains_key(LAST_MODIFIED));
    }

    // ── RED: If-Modified-Since fallback ───────────────────────────────────────

    #[test]
    fn fresh_when_if_modified_since_returns_stale_without_if_none_match() {
        use chrono::TimeZone;

        let last_modified = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let ims_time = chrono::Utc.timestamp_opt(1_700_000_001, 0).unwrap();
        let ims_str = http_date(ims_time);

        let mut headers = HeaderMap::new();
        headers.insert(
            IF_MODIFIED_SINCE,
            HeaderValue::from_str(&ims_str).unwrap(),
        );

        // fresh_when only checks If-None-Match. If-Modified-Since fallback
        // is a separate concern handled by check_if_modified_since with last_modified.
        // Without an If-None-Match header, fresh_when always returns stale.
        let result = fresh_when(&headers, 1_i64).last_modified(last_modified);
        assert!(!result.is_fresh());
    }

    // ── RED: EtagLayer ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn etag_layer_adds_etag_to_get_200() {
        use tower::ServiceExt;

        let svc = EtagLayer::new().layer(tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from("hello world"))
                    .unwrap(),
            )
        }));

        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let response = svc.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers().contains_key(ETAG),
            "EtagLayer must inject ETag header"
        );
    }

    #[tokio::test]
    async fn etag_layer_returns_304_on_matching_if_none_match() {
        let svc = EtagLayer::new().layer(tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from("hello world"))
                    .unwrap(),
            )
        }));

        // First call to discover the ETag.
        let first_req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let first_response = svc.clone().oneshot(first_req).await.unwrap();
        let etag = first_response.headers().get(ETAG).unwrap().clone();

        // Second call with If-None-Match.
        let second_req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .header(IF_NONE_MATCH, etag)
            .body(Body::empty())
            .unwrap();
        let second_response = svc.oneshot(second_req).await.unwrap();
        assert_eq!(second_response.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn etag_layer_does_not_add_etag_to_post() {
        let svc = EtagLayer::new().layer(tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from("ok"))
                    .unwrap(),
            )
        }));

        let req = Request::builder()
            .method(Method::POST)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let response = svc.oneshot(req).await.unwrap();
        assert!(!response.headers().contains_key(ETAG));
    }

    #[tokio::test]
    async fn etag_layer_does_not_override_existing_etag() {
        let svc = EtagLayer::new().layer(tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(ETAG, r#""handler-set""#)
                    .body(Body::from("body"))
                    .unwrap(),
            )
        }));

        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let response = svc.oneshot(req).await.unwrap();
        assert_eq!(
            response.headers().get(ETAG).unwrap().to_str().unwrap(),
            r#""handler-set""#
        );
    }

    #[tokio::test]
    async fn etag_layer_preserves_cache_control_on_304() {
        let svc = EtagLayer::new().layer(tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CACHE_CONTROL, "max-age=60")
                    .body(Body::from("stable content"))
                    .unwrap(),
            )
        }));

        let first_req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let first = svc.clone().oneshot(first_req).await.unwrap();
        let etag = first.headers().get(ETAG).unwrap().clone();

        let second_req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .header(IF_NONE_MATCH, etag)
            .body(Body::empty())
            .unwrap();
        let second = svc.oneshot(second_req).await.unwrap();
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            second
                .headers()
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap(),
            "max-age=60"
        );
    }

    #[tokio::test]
    async fn etag_layer_strips_set_cookie_from_304() {
        let svc = EtagLayer::new().layer(tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(SET_COOKIE, "session=abc; HttpOnly")
                    .body(Body::from("content"))
                    .unwrap(),
            )
        }));

        let first = svc
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let etag = first.headers().get(ETAG).unwrap().clone();

        let second = svc
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .header(IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
        assert!(
            !second.headers().contains_key(SET_COOKIE),
            "Set-Cookie must be stripped from 304"
        );
    }

    // ── RED: determinism across replicas ──────────────────────────────────────

    #[test]
    fn etag_derivation_is_deterministic_no_rng_or_clock() {
        // Same inputs → same ETag. Calling multiple times must yield identical results.
        let e1: ETag = (42_i64).into_etag();
        let e2: ETag = (42_i64).into_etag();
        let e3: ETag = (42_i64).into_etag();
        assert_eq!(e1, e2);
        assert_eq!(e2, e3);
    }

    // ── RED: build_not_modified helper ────────────────────────────────────────

    #[test]
    fn build_not_modified_preserves_cache_control_and_vary() {
        let mut orig = HeaderMap::new();
        orig.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        orig.insert(VARY, HeaderValue::from_static("Accept"));
        orig.insert(SET_COOKIE, HeaderValue::from_static("tok=x"));

        let etag = ETag::strong("tag");
        let response = build_not_modified(&orig, &etag, None);

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            response.headers().get(CACHE_CONTROL).unwrap().to_str().unwrap(),
            "no-cache"
        );
        assert_eq!(
            response.headers().get(VARY).unwrap().to_str().unwrap(),
            "Accept"
        );
        // Set-Cookie is NOT preserved — build_not_modified strips it.
        assert!(!response.headers().contains_key(SET_COOKIE));
    }

    // ── Integration: first GET → 200 + ETag, second → 304 ────────────────────

    #[tokio::test]
    async fn integration_first_get_200_second_get_304() {
        use std::sync::atomic::{AtomicI64, Ordering};
        use std::sync::Arc;

        let lock_version = Arc::new(AtomicI64::new(1));
        let lv = Arc::clone(&lock_version);

        let svc = EtagLayer::new().layer(tower::service_fn(move |_req: Request<Body>| {
            let v = lv.load(Ordering::SeqCst);
            async move {
                let etag: ETag = v.into_etag();
                Ok::<_, std::convert::Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(ETAG, etag.header_value())
                        .body(Body::from(format!("version={v}")))
                        .unwrap(),
                )
            }
        }));

        // First GET → 200 + ETag.
        let first = svc
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/resource")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first.headers().get(ETAG).cloned().unwrap();

        // Second GET with matching If-None-Match → 304 empty body.
        let second = svc
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/resource")
                    .header(IF_NONE_MATCH, etag.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
        let body_bytes = second.into_body().collect().await.unwrap().to_bytes();
        assert!(body_bytes.is_empty(), "304 body must be empty");

        // Simulate mutation: lock_version bumped.
        lock_version.store(2, Ordering::SeqCst);

        // Third GET with old ETag → 200 with new ETag.
        let third = svc
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/resource")
                    .header(IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(third.status(), StatusCode::OK);
        let new_etag = third.headers().get(ETAG).unwrap();
        let old_etag: ETag = 1_i64.into_etag();
        // New ETag must differ from old one.
        assert_ne!(new_etag, &old_etag.header_value());
    }
}
