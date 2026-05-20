//! Traced outbound HTTP client with retries and test mocks.
//!
//! Exposes [`Client`](crate::http_client::Client) as `autumn_web::http::Client` — a thin `reqwest`-backed
//! outbound HTTP client that propagates the active span's `traceparent` /
//! `tracestate` headers, retries transient failures, and is mockable in tests
//! via [`TestApp::http_mock`](crate::test::TestApp::http_mock).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use autumn_web::http::Client;
//!
//! #[post("/pay")]
//! async fn pay(client: Client) -> AutumnResult<Json<serde_json::Value>> {
//!     let resp = client
//!         .post("https://api.stripe.com/v1/charges")
//!         .header("authorization", "Bearer sk_test_xxx")
//!         .json(&serde_json::json!({"amount": 1000, "currency": "usd"}))
//!         .send()
//!         .await?;
//!     Ok(Json(resp.json()?))
//! }
//! ```
//!
//! # Test mocks
//!
//! ```rust,no_run
//! use autumn_web::test::TestApp;
//! use autumn_web::prelude::*;
//! use serde_json::json;
//!
//! // (handler shown above)
//!
//! #[tokio::test]
//! async fn pay_calls_stripe() {
//!     let mut app = TestApp::new().routes(routes![pay]);
//!     let mock = app.http_mock("stripe")
//!         .post("/v1/charges")
//!         .respond_with(200, json!({"id": "ch_123", "amount": 1000}));
//!
//!     let client = app.build();
//!     client.post("/pay").send().await.assert_status(200);
//!     mock.expect_called(1);
//! }
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use serde::de::DeserializeOwned;

// ── Error ────────────────────────────────────────────────────────────────────

/// Errors produced by [`Client`] and [`RequestBuilder`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// An underlying `reqwest` transport error.
    #[error("outbound HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// JSON (de)serialisation failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// No mock entry matched the outgoing request.
    #[error("no mock registered for {0} {1}")]
    NoMock(String, String),
}

// ── Response ─────────────────────────────────────────────────────────────────

/// Completed outbound HTTP response with eagerly-collected body bytes.
///
/// Body is consumed once — call exactly one of [`json`](Self::json),
/// [`text`](Self::text), or [`bytes`](Self::bytes).
pub struct Response {
    status: reqwest::StatusCode,
    headers: HeaderMap,
    body: Bytes,
    url: Option<reqwest::Url>,
}

impl Response {
    /// HTTP status code.
    pub const fn status(&self) -> reqwest::StatusCode {
        self.status
    }

    /// Response headers (sensitive values are **not** redacted here).
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// `true` when the status code is in the 2xx range.
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    /// URL that was ultimately requested (after redirects, if any).
    pub const fn url(&self) -> Option<&reqwest::Url> {
        self.url.as_ref()
    }

    /// Deserialise the body as JSON.
    ///
    /// # Errors
    /// Returns [`ClientError::Json`] if the body is not valid JSON for `T`.
    pub fn json<T: DeserializeOwned>(self) -> Result<T, ClientError> {
        serde_json::from_slice(&self.body).map_err(ClientError::Json)
    }

    /// Return the body as a UTF-8 string (lossy).
    pub fn text(self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Return the raw body bytes.
    pub fn bytes(self) -> Bytes {
        self.body
    }
}

// ── RetryPolicy ──────────────────────────────────────────────────────────────

/// Retry configuration for a [`RequestBuilder`].
#[derive(Clone, Debug)]
pub struct RetryPolicy {
    /// Maximum number of additional attempts after the first failure.  Zero
    /// means no retries (one attempt total).
    pub max_retries: u32,
    /// When `true` (the default), only GET / HEAD / PUT / DELETE / OPTIONS /
    /// TRACE are retried; POST and PATCH are not.
    pub retry_idempotent_only: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_idempotent_only: true,
        }
    }
}

// ── MockRegistry ─────────────────────────────────────────────────────────────

/// Internal mock entry stored by [`MockRegistry`].
pub(crate) struct MockEntry {
    pub(crate) method: Option<Method>,
    /// URL path to match against the path component of the outbound URL.
    pub(crate) path: String,
    /// Optional alias that must match the `Client`'s alias.
    pub(crate) alias: Option<String>,
    pub(crate) status: u16,
    pub(crate) body: Option<serde_json::Value>,
    pub(crate) call_count: Arc<AtomicUsize>,
}

/// Canned response returned by a [`MockRegistry`] match.
pub(crate) struct MockResponse {
    pub(crate) status: u16,
    pub(crate) body: Option<serde_json::Value>,
}

/// In-process mock registry used by [`TestApp::http_mock`](crate::test::TestApp::http_mock).
///
/// Stored in [`AppState`](crate::AppState) extensions during test builds so
/// that any [`Client`] extracted from state will intercept matching requests
/// and return canned responses without hitting the network.
pub struct MockRegistry {
    entries: Mutex<Vec<MockEntry>>,
}

impl MockRegistry {
    /// Create an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Register a new mock entry.
    pub(crate) fn register(&self, entry: MockEntry) {
        self.entries
            .lock()
            .expect("mock registry lock poisoned")
            .push(entry);
    }

    /// Find the first entry matching `(method, url, alias)` and increment its
    /// call counter.  Returns `None` when no entry matches.
    pub(crate) fn find_match(
        &self,
        method: &Method,
        url: &str,
        alias: Option<&str>,
    ) -> Option<MockResponse> {
        // Extract the URL path component for precise matching.
        // For full URLs (https://…) we parse and use the path segment.
        // For relative paths we use the raw string as-is.
        let url_path_owned: Option<String> =
            reqwest::Url::parse(url).ok().map(|u| u.path().to_owned());
        let url_path = url_path_owned.as_deref().unwrap_or(url);

        // Hold the lock only for the search; release before fetching metadata.
        let found = {
            let entries = self.entries.lock().expect("mock registry lock poisoned");
            entries.iter().find_map(|entry| {
                let method_ok = entry.method.as_ref().is_none_or(|m| m == method);
                // Path match: exact equality OR suffix at a segment boundary.
                let path_ok = url_path == entry.path.as_str()
                    || url_path
                        .strip_suffix(entry.path.as_str())
                        .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('/'));
                let alias_ok = entry
                    .alias
                    .as_deref()
                    .is_none_or(|a| alias.is_some_and(|b| a == b));
                if method_ok && path_ok && alias_ok {
                    Some((entry.call_count.clone(), entry.status, entry.body.clone()))
                } else {
                    None
                }
            })
        };

        found.map(|(call_count, status, body)| {
            call_count.fetch_add(1, Ordering::SeqCst);
            MockResponse { status, body }
        })
    }
}

impl Default for MockRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Newtype stored in [`AppState`](crate::AppState) extensions so the
/// `MockRegistry` `Arc` survives a `build()` without double-wrapping.
pub struct HttpMockRegistryExt(pub Arc<MockRegistry>);

/// Handle returned by
/// [`MockSetupBuilder::respond_with`] that lets tests assert call counts.
pub struct MockHandle {
    alias: String,
    method: String,
    path: String,
    call_count: Arc<AtomicUsize>,
}

impl MockHandle {
    /// Assert that the mocked endpoint was called exactly `expected` times.
    ///
    /// # Panics
    ///
    /// Panics with a diagnostic message when the actual call count differs.
    pub fn expect_called(&self, expected: usize) {
        let actual = self.call_count.load(Ordering::SeqCst);
        assert_eq!(
            actual, expected,
            "http mock for {} {} {} expected {} call(s) but got {}",
            self.alias, self.method, self.path, expected, actual,
        );
    }

    /// Return the raw call count without asserting.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

/// Builder returned by [`TestApp::http_mock`](crate::test::TestApp::http_mock).
///
/// Chain a method call (`get`, `post`, …) and a path, then call
/// [`respond_with`](Self::respond_with) to register the entry and obtain a
/// [`MockHandle`] for later assertions.
pub struct MockSetupBuilder {
    pub(crate) registry: Arc<MockRegistry>,
    pub(crate) alias: String,
    pub(crate) method: Option<Method>,
    pub(crate) path: Option<String>,
}

impl MockSetupBuilder {
    /// Match `GET <path>`.
    #[must_use]
    pub fn get(mut self, path: &str) -> Self {
        self.method = Some(Method::GET);
        self.path = Some(path.to_owned());
        self
    }
    /// Match `POST <path>`.
    #[must_use]
    pub fn post(mut self, path: &str) -> Self {
        self.method = Some(Method::POST);
        self.path = Some(path.to_owned());
        self
    }
    /// Match `PUT <path>`.
    #[must_use]
    pub fn put(mut self, path: &str) -> Self {
        self.method = Some(Method::PUT);
        self.path = Some(path.to_owned());
        self
    }
    /// Match `PATCH <path>`.
    #[must_use]
    pub fn patch(mut self, path: &str) -> Self {
        self.method = Some(Method::PATCH);
        self.path = Some(path.to_owned());
        self
    }
    /// Match `DELETE <path>`.
    #[must_use]
    pub fn delete(mut self, path: &str) -> Self {
        self.method = Some(Method::DELETE);
        self.path = Some(path.to_owned());
        self
    }

    /// Register the mock entry and return a [`MockHandle`] for assertions.
    ///
    /// `status` is the HTTP status code to return.
    /// `body` is serialised as JSON and returned as the response body.
    #[must_use]
    pub fn respond_with(self, status: u16, body: serde_json::Value) -> MockHandle {
        let path = self.path.clone().unwrap_or_default();
        let method_str = self
            .method
            .as_ref()
            .map_or_else(|| "*".to_owned(), ToString::to_string);
        let call_count = Arc::new(AtomicUsize::new(0));

        self.registry.register(MockEntry {
            method: self.method,
            path: path.clone(),
            alias: Some(self.alias.clone()),
            status,
            body: Some(body),
            call_count: call_count.clone(),
        });

        MockHandle {
            alias: self.alias,
            method: method_str,
            path,
            call_count,
        }
    }

    /// Convenience variant that returns the given status with an empty body.
    ///
    /// Unlike [`respond_with`](Self::respond_with), this stores `body: None` so
    /// the mock response truly has zero body bytes (not the JSON literal `null`).
    #[must_use]
    pub fn respond_with_status(self, status: u16) -> MockHandle {
        let path = self.path.clone().unwrap_or_default();
        let method_str = self
            .method
            .as_ref()
            .map_or_else(|| "*".to_owned(), ToString::to_string);
        let call_count = Arc::new(AtomicUsize::new(0));

        self.registry.register(MockEntry {
            method: self.method,
            path: path.clone(),
            alias: Some(self.alias.clone()),
            status,
            body: None,
            call_count: call_count.clone(),
        });

        MockHandle {
            alias: self.alias,
            method: method_str,
            path,
            call_count,
        }
    }
}

// ── Client ───────────────────────────────────────────────────────────────────

/// Traced outbound HTTP client with automatic retries and test-mock support.
///
/// Extracted from `AppState` via Axum's extractor machinery — declare it as a
/// handler parameter to get a pre-configured instance that respects
/// `[http.client]` config and, in test builds, intercepts requests against any
/// registered mocks.
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::http::Client;
///
/// #[get("/ping-upstream")]
/// async fn ping(client: Client) -> AutumnResult<&'static str> {
///     client.get("https://api.example.com/health").send().await?;
///     Ok("ok")
/// }
/// ```
///
/// You can also construct a standalone client outside of a handler:
///
/// ```rust
/// use autumn_web::http::Client;
///
/// let client = Client::new();
/// ```
#[derive(Clone)]
pub struct Client {
    inner: reqwest::Client,
    /// Named alias — used to look up base URLs from config and to match mocks.
    alias: Option<String>,
    /// Base URL prepended to relative paths.
    base_url: Option<String>,
    /// Alias → base URL map loaded from `[http.client.base_urls]` config.
    base_urls: HashMap<String, String>,
    retry_policy: RetryPolicy,
    /// When present (test builds), matching requests bypass the network.
    mock: Option<Arc<MockRegistry>>,
}

impl Client {
    /// Create a new client with default settings (30 s timeout, 3 retries on
    /// idempotent methods).
    #[must_use]
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Create a client with a custom per-request timeout.
    ///
    /// # Panics
    ///
    /// Panics if the underlying TLS backend cannot be initialised (should not
    /// happen with the default `rustls-tls` feature).
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        let inner = reqwest::ClientBuilder::new()
            .timeout(timeout)
            .build()
            .expect("failed to build reqwest client");
        Self {
            inner,
            alias: None,
            base_url: None,
            base_urls: HashMap::new(),
            retry_policy: RetryPolicy::default(),
            mock: None,
        }
    }

    /// Create a client from `[http.client]` framework configuration.
    ///
    /// # Panics
    ///
    /// Panics if the underlying TLS backend cannot be initialised (should not
    /// happen with the default `rustls-tls` feature).
    #[must_use]
    pub fn from_config(config: &crate::config::HttpClientConfig) -> Self {
        let inner = reqwest::ClientBuilder::new()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build reqwest client");
        Self {
            inner,
            alias: None,
            base_url: None,
            base_urls: config.base_urls.clone(),
            retry_policy: RetryPolicy {
                max_retries: config.max_retries,
                retry_idempotent_only: true,
            },
            mock: None,
        }
    }

    /// Attach a mock registry (used by the test harness).
    pub(crate) fn with_mock(mut self, registry: Arc<MockRegistry>) -> Self {
        self.mock = Some(registry);
        self
    }

    /// Return a clone of this client scoped to the named alias.
    ///
    /// When a `[http.client.base_urls]` entry exists for the alias the client
    /// will prepend that URL to all relative paths. Mocks registered for the
    /// alias via [`TestApp::http_mock`](crate::test::TestApp::http_mock) will
    /// match requests made through this named client.
    #[must_use]
    pub fn named(&self, alias: &str) -> Self {
        let base_url = self
            .base_urls
            .get(alias)
            .cloned()
            .or_else(|| self.base_url.clone());
        Self {
            inner: self.inner.clone(),
            alias: Some(alias.to_owned()),
            base_url,
            base_urls: self.base_urls.clone(),
            retry_policy: self.retry_policy.clone(),
            mock: self.mock.clone(),
        }
    }

    /// Set (or override) the base URL prepended to relative request paths.
    #[must_use]
    pub fn with_base_url(&self, base_url: impl Into<String>) -> Self {
        Self {
            inner: self.inner.clone(),
            alias: self.alias.clone(),
            base_url: Some(base_url.into()),
            base_urls: self.base_urls.clone(),
            retry_policy: self.retry_policy.clone(),
            mock: self.mock.clone(),
        }
    }

    fn build_request(&self, method: Method, url: impl AsRef<str>) -> RequestBuilder {
        let url_str = url.as_ref();
        let full_url = if url_str.starts_with("http://") || url_str.starts_with("https://") {
            url_str.to_owned()
        } else if let Some(base) = &self.base_url {
            format!(
                "{}/{}",
                base.trim_end_matches('/'),
                url_str.trim_start_matches('/')
            )
        } else {
            url_str.to_owned()
        };

        RequestBuilder {
            client: self.inner.clone(),
            method,
            url: full_url,
            extra_headers: HeaderMap::new(),
            body: None,
            retry_policy: self.retry_policy.clone(),
            mock: self.mock.clone(),
            alias: self.alias.clone(),
            pending_error: None,
        }
    }

    /// Build a `GET` request.
    #[must_use]
    pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.build_request(Method::GET, url)
    }
    /// Build a `POST` request.
    #[must_use]
    pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.build_request(Method::POST, url)
    }
    /// Build a `PUT` request.
    #[must_use]
    pub fn put(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.build_request(Method::PUT, url)
    }
    /// Build a `PATCH` request.
    #[must_use]
    pub fn patch(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.build_request(Method::PATCH, url)
    }
    /// Build a `DELETE` request.
    #[must_use]
    pub fn delete(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.build_request(Method::DELETE, url)
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl axum::extract::FromRequestParts<crate::AppState> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        _parts: &mut http::request::Parts,
        state: &crate::AppState,
    ) -> Result<Self, std::convert::Infallible> {
        // Check for an explicit HttpConfig extension first (inserted by TestApp::build());
        // in production, fall back to the full AutumnConfig's http section.
        let config = state.extension::<crate::config::HttpConfig>().or_else(|| {
            state
                .extension::<crate::config::AutumnConfig>()
                .map(|c| std::sync::Arc::new(c.http.clone()))
        });
        let mut client = config.map_or_else(Self::new, |cfg| Self::from_config(&cfg.client));

        // In test builds the mock registry is installed by TestApp::build().
        if let Some(ext) = state.extension::<HttpMockRegistryExt>() {
            client = client.with_mock(ext.0.clone());
        }

        Ok(client)
    }
}

// ── RequestBuilder ───────────────────────────────────────────────────────────

/// Fluent outbound request builder produced by [`Client`] methods.
pub struct RequestBuilder {
    client: reqwest::Client,
    method: Method,
    url: String,
    extra_headers: HeaderMap,
    /// Request body. `Bytes` gives O(1) clones across retry attempts.
    body: Option<Bytes>,
    retry_policy: RetryPolicy,
    mock: Option<Arc<MockRegistry>>,
    alias: Option<String>,
    /// Captures errors from `json()` or invalid headers to surface in `send()`.
    pending_error: Option<ClientError>,
}

impl RequestBuilder {
    /// Append a request header.
    ///
    /// Headers named `authorization`, `cookie`, or `set-cookie` are accepted
    /// normally but are **redacted** in tracing events and log output.
    /// Invalid header names or values emit a `tracing::warn!` and are skipped.
    #[must_use]
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let name_str = name.as_ref();
        let value_str = value.as_ref();
        match (
            HeaderName::from_bytes(name_str.as_bytes()),
            HeaderValue::from_str(value_str),
        ) {
            (Ok(n), Ok(v)) => {
                self.extra_headers.insert(n, v);
            }
            (Err(e), _) => {
                tracing::warn!(header.name = name_str, error = %e, "invalid header name — header skipped");
            }
            (_, Err(e)) => {
                tracing::warn!(header.name = name_str, error = %e, "invalid header value — header skipped");
            }
        }
        self
    }

    /// Serialise `body` as JSON and set `Content-Type: application/json`.
    ///
    /// Serialisation errors are captured and returned when [`send`](Self::send)
    /// is called rather than being silently discarded.
    #[must_use]
    pub fn json<T: Serialize>(mut self, body: &T) -> Self {
        match serde_json::to_vec(body) {
            Ok(bytes) => {
                self.body = Some(Bytes::from(bytes));
                self = self.header("content-type", "application/json");
            }
            Err(e) => {
                self.pending_error = Some(ClientError::Json(e));
            }
        }
        self
    }

    /// Set a plain-text body.
    #[must_use]
    pub fn text_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(Bytes::from(body.into().into_bytes()));
        self
    }

    /// Override the maximum retry count for this request.
    #[must_use]
    pub const fn retries(mut self, max: u32) -> Self {
        self.retry_policy.max_retries = max;
        self
    }

    /// Disable retries for this request.
    #[must_use]
    pub const fn no_retry(mut self) -> Self {
        self.retry_policy.max_retries = 0;
        self
    }

    /// Send the request, applying retries and returning a [`Response`].
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Json`] if a prior `.json()` call failed to
    /// serialise the body.  Returns [`ClientError::Request`] for transport
    /// errors that exhaust all retry attempts.  Returns [`ClientError::NoMock`]
    /// if the request is made in a test context without a matching mock entry.
    ///
    /// # Panics
    ///
    /// Contains an internal `unreachable!()` that guards against a logic error
    /// in the retry loop; it cannot be reached in practice.
    pub async fn send(self) -> Result<Response, ClientError> {
        // Surface any error captured during builder construction.
        if let Some(err) = self.pending_error {
            return Err(err);
        }

        // ── Mock short-circuit ──────────────────────────────────────────────
        if let Some(ref mock) = self.mock {
            match mock.find_match(&self.method, &self.url, self.alias.as_deref()) {
                Some(mock_resp) => {
                    let status = reqwest::StatusCode::from_u16(mock_resp.status)
                        .unwrap_or(reqwest::StatusCode::OK);
                    let body_bytes = mock_resp
                        .body
                        .as_ref()
                        .map(|v| serde_json::to_vec(v).unwrap_or_default())
                        .unwrap_or_default();

                    tracing::info!(
                        http.method = %self.method,
                        http.url = %self.url,
                        http.status = mock_resp.status,
                        "[mock] outbound request intercepted"
                    );

                    return Ok(Response {
                        status,
                        headers: HeaderMap::new(),
                        body: Bytes::from(body_bytes),
                        url: None,
                    });
                }
                None => {
                    // A mock registry is present but nothing matched — treat as
                    // a test failure rather than falling through to the network.
                    return Err(ClientError::NoMock(
                        self.method.to_string(),
                        self.url.clone(),
                    ));
                }
            }
        }

        // ── Real network request with retries ───────────────────────────────
        let start = Instant::now();
        let max_attempts =
            if is_idempotent_method(&self.method) || !self.retry_policy.retry_idempotent_only {
                self.retry_policy.max_retries.saturating_add(1)
            } else {
                1
            };

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay = Duration::from_millis(100 * 2_u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
            }

            let mut req = self.client.request(self.method.clone(), &self.url);

            // Inject W3C trace context headers from the active span.
            req = inject_trace_context(req);

            // Apply caller-supplied headers (may override or extend trace headers).
            for (name, value) in &self.extra_headers {
                req = req.header(name.clone(), value.clone());
            }

            if let Some(body) = &self.body {
                req = req.body(body.clone());
            }

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    let url_used = resp.url().clone();

                    // 429 → honour Retry-After and retry if attempts remain.
                    if status.as_u16() == 429 && attempt + 1 < max_attempts {
                        if let Some(delay) = parse_retry_after(&headers) {
                            tokio::time::sleep(delay).await;
                        } else {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        continue;
                    }

                    // 5xx transient gateway errors → retry if attempts remain.
                    if is_retryable_status(status.as_u16()) && attempt + 1 < max_attempts {
                        continue;
                    }

                    let body = resp.bytes().await.map_err(ClientError::Request)?;
                    let elapsed = start.elapsed();
                    log_request(
                        self.method.as_str(),
                        &url_used,
                        status.as_u16(),
                        elapsed,
                        &self.extra_headers,
                    );

                    return Ok(Response {
                        status,
                        headers,
                        body,
                        url: Some(url_used),
                    });
                }
                // Retry on connect/timeout errors while attempts remain.
                Err(_) if attempt + 1 < max_attempts => {}
                Err(e) => return Err(ClientError::Request(e)),
            }
        }

        // The retry loop always returns inside the last attempt; this is unreachable.
        unreachable!("retry loop exited without returning a result — this is a bug")
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

const fn is_idempotent_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}

const fn is_retryable_status(status: u16) -> bool {
    matches!(status, 502..=504)
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get("retry-after")?.to_str().ok()?;
    // Integer seconds (most common form).
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // HTTP-date format per RFC 9110 (e.g. "Tue, 01 Jan 2030 00:00:00 GMT").
    let dt = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let now = chrono::Utc::now();
    let future = dt.with_timezone(&chrono::Utc);
    let secs = u64::try_from((future - now).num_seconds().max(0)).unwrap_or(0);
    Some(Duration::from_secs(secs))
}

const REDACTED_HEADERS: &[&str] = &["authorization", "cookie", "set-cookie"];

fn is_sensitive_header(name: &str) -> bool {
    REDACTED_HEADERS
        .iter()
        .any(|h| h.eq_ignore_ascii_case(name))
}

fn log_request(
    method: &str,
    url: &reqwest::Url,
    status: u16,
    elapsed: Duration,
    headers: &HeaderMap,
) {
    let host = url.host_str().unwrap_or("unknown");
    let path = url.path();

    // Collect non-sensitive header names for the span (values are omitted).
    let sent_headers: Vec<&str> = headers
        .keys()
        .map(HeaderName::as_str)
        .filter(|k| !is_sensitive_header(k))
        .collect();

    tracing::info!(
        http.method = method,
        http.host = host,
        http.path = path,
        http.status = status,
        http.elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        http.sent_headers = ?sent_headers,
        "outbound request"
    );
}

/// Inject the active span's W3C `traceparent` / `tracestate` headers into the
/// request builder.  No-ops when the `telemetry-otlp` feature is disabled or
/// when there is no active span with a valid context.
#[allow(clippy::missing_const_for_fn)]
fn inject_trace_context(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    #[cfg(not(feature = "telemetry-otlp"))]
    {
        builder
    }
    #[cfg(feature = "telemetry-otlp")]
    {
        use std::collections::HashMap;
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        let cx = tracing::Span::current().context();
        let mut map = HashMap::<String, String>::new();
        opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&cx, &mut TraceHeaderInjector(&mut map));
        });
        let mut builder = builder;
        for (k, v) in map {
            if let Ok(value) = HeaderValue::from_str(&v) {
                builder = builder.header(k, value);
            }
        }
        builder
    }
}

#[cfg(feature = "telemetry-otlp")]
struct TraceHeaderInjector<'a>(&'a mut std::collections::HashMap<String, String>);

#[cfg(feature = "telemetry-otlp")]
impl opentelemetry::propagation::Injector for TraceHeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpClientConfig;

    // RED-PHASE TEST 1: Client can be constructed with defaults.
    #[test]
    fn client_constructs_with_defaults() {
        let client = Client::new();
        assert!(client.alias.is_none());
        assert!(client.base_url.is_none());
        assert_eq!(client.retry_policy.max_retries, 3);
    }

    // RED-PHASE TEST 2: Fluent RequestBuilder API compiles.
    #[test]
    fn request_builder_fluent_api_compiles() {
        let client = Client::new();
        let _builder = client
            .post("https://example.com/api")
            .header("x-api-key", "secret")
            .json(&serde_json::json!({"key": "value"}))
            .retries(2);
    }

    // RED-PHASE TEST 3: Response accessors work.
    #[test]
    fn response_accessors_work() {
        let payload = serde_json::json!({"id": 42, "name": "Alice"});
        let body = serde_json::to_vec(&payload).unwrap();
        let resp = Response {
            status: reqwest::StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from(body),
            url: None,
        };
        assert_eq!(resp.status().as_u16(), 200);
        assert!(resp.is_success());
    }

    // RED-PHASE TEST 4: Response::json() deserialises correctly.
    #[test]
    fn response_json_deserialises() {
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct User {
            id: i32,
            name: String,
        }
        let payload = serde_json::json!({"id": 1, "name": "Bob"});
        let resp = Response {
            status: reqwest::StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from(serde_json::to_vec(&payload).unwrap()),
            url: None,
        };
        let user: User = resp.json().unwrap();
        assert_eq!(user.id, 1);
        assert_eq!(user.name, "Bob");
    }

    // RED-PHASE TEST 5: Response::text() returns UTF-8 string.
    #[test]
    fn response_text_returns_string() {
        let resp = Response {
            status: reqwest::StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"hello world"),
            url: None,
        };
        assert_eq!(resp.text(), "hello world");
    }

    // RED-PHASE TEST 6: Response::bytes() returns raw bytes.
    #[test]
    fn response_bytes_returns_raw() {
        let resp = Response {
            status: reqwest::StatusCode::CREATED,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"\x00\x01\x02"),
            url: None,
        };
        assert_eq!(resp.bytes(), Bytes::from_static(b"\x00\x01\x02"));
    }

    // RED-PHASE TEST 7: HttpClientConfig deserialises from [http.client] TOML.
    #[test]
    fn config_deserialises_from_toml() {
        // Simulate the [http.client] section as it appears in autumn.toml.
        let toml = r#"
            [client]
            timeout_secs = 60
            max_retries = 5
            [client.base_urls]
            stripe = "https://api.stripe.com"
            sendgrid = "https://api.sendgrid.com"
        "#;
        let http_cfg: crate::config::HttpConfig = toml::from_str(toml).unwrap();
        let config = &http_cfg.client;
        assert_eq!(config.timeout_secs, 60);
        assert_eq!(config.max_retries, 5);
        assert_eq!(
            config.base_urls.get("stripe").map(String::as_str),
            Some("https://api.stripe.com")
        );
        assert_eq!(
            config.base_urls.get("sendgrid").map(String::as_str),
            Some("https://api.sendgrid.com")
        );
    }

    // RED-PHASE TEST 8: HttpClientConfig has correct defaults.
    #[test]
    fn config_has_correct_defaults() {
        let config = HttpClientConfig::default();
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_retries, 3);
        assert!(config.base_urls.is_empty());
    }

    // RED-PHASE TEST 9: is_idempotent_method returns correct values.
    #[test]
    fn idempotent_method_classification() {
        assert!(is_idempotent_method(&Method::GET));
        assert!(is_idempotent_method(&Method::HEAD));
        assert!(is_idempotent_method(&Method::PUT));
        assert!(is_idempotent_method(&Method::DELETE));
        assert!(is_idempotent_method(&Method::OPTIONS));
        assert!(is_idempotent_method(&Method::TRACE));
        assert!(!is_idempotent_method(&Method::POST));
        assert!(!is_idempotent_method(&Method::PATCH));
    }

    // RED-PHASE TEST 10: is_retryable_status returns correct values.
    #[test]
    fn retryable_status_classification() {
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(504));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(500));
        assert!(!is_retryable_status(429));
    }

    // RED-PHASE TEST 11: parse_retry_after parses seconds correctly.
    #[test]
    fn retry_after_header_parsing() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::HeaderName::from_static("retry-after"),
            HeaderValue::from_static("5"),
        );
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(5)));

        let empty = HeaderMap::new();
        assert_eq!(parse_retry_after(&empty), None);
    }

    // RED-PHASE TEST 12: Sensitive header detection.
    #[test]
    fn sensitive_header_detection() {
        assert!(is_sensitive_header("authorization"));
        assert!(is_sensitive_header("Authorization"));
        assert!(is_sensitive_header("AUTHORIZATION"));
        assert!(is_sensitive_header("cookie"));
        assert!(is_sensitive_header("set-cookie"));
        assert!(!is_sensitive_header("content-type"));
        assert!(!is_sensitive_header("x-api-key"));
    }

    // RED-PHASE TEST 13: MockRegistry captures and matches calls.
    #[tokio::test]
    async fn mock_registry_captures_calls() {
        let registry = Arc::new(MockRegistry::new());
        let call_count = Arc::new(AtomicUsize::new(0));

        registry.register(MockEntry {
            method: Some(Method::POST),
            path: "/charges".to_owned(),
            alias: Some("stripe".to_owned()),
            status: 200,
            body: Some(serde_json::json!({"id": "ch_123"})),
            call_count: call_count.clone(),
        });

        let client = Client::new().with_mock(registry).named("stripe");

        let resp = client
            .post("https://api.stripe.com/charges")
            .json(&serde_json::json!({"amount": 1000}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 200);
        let body: serde_json::Value = resp.json().unwrap();
        assert_eq!(body["id"], "ch_123");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // RED-PHASE TEST 14: MockHandle::expect_called passes when count matches.
    #[tokio::test]
    async fn mock_handle_expect_called_passes() {
        let registry = Arc::new(MockRegistry::new());
        let call_count = Arc::new(AtomicUsize::new(0));

        registry.register(MockEntry {
            method: Some(Method::GET),
            path: "/users/1".to_owned(),
            alias: None,
            status: 200,
            body: Some(serde_json::json!({"name": "Alice"})),
            call_count: call_count.clone(),
        });

        let handle = MockHandle {
            alias: "test".to_owned(),
            method: "GET".to_owned(),
            path: "/users/1".to_owned(),
            call_count: call_count.clone(),
        };

        let client = Client::new().with_mock(registry);
        client
            .get("https://api.example.com/users/1")
            .send()
            .await
            .unwrap();

        handle.expect_called(1);
        assert_eq!(handle.call_count(), 1);
    }

    // RED-PHASE TEST 15: MockRegistry matches by URL path suffix.
    #[tokio::test]
    async fn mock_matches_by_path_suffix() {
        let registry = Arc::new(MockRegistry::new());
        let call_count = Arc::new(AtomicUsize::new(0));

        registry.register(MockEntry {
            method: Some(Method::POST),
            path: "/v1/charges".to_owned(),
            alias: None,
            status: 201,
            body: Some(serde_json::json!({"created": true})),
            call_count: call_count.clone(),
        });

        let client = Client::new().with_mock(registry);
        let resp = client
            .post("https://api.stripe.com/v1/charges")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 201);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // RED-PHASE TEST 16: NoMock error when mock registry has no match.
    #[tokio::test]
    async fn no_mock_error_when_unmatched() {
        let registry = Arc::new(MockRegistry::new());
        let client = Client::new().with_mock(registry);
        let result = client.post("https://api.example.com/unknown").send().await;
        assert!(matches!(result, Err(ClientError::NoMock(_, _))));
    }

    // RED-PHASE TEST 17: MockSetupBuilder registers and returns MockHandle.
    #[tokio::test]
    async fn mock_setup_builder_registers_entry() {
        let registry = Arc::new(MockRegistry::new());
        let builder = MockSetupBuilder {
            registry: registry.clone(),
            alias: "myservice".to_owned(),
            method: None,
            path: None,
        };

        let handle = builder
            .post("/api/resource")
            .respond_with(201, serde_json::json!({"ok": true}));

        let client = Client::new().with_mock(registry).named("myservice");
        client
            .post("https://myservice.example.com/api/resource")
            .send()
            .await
            .unwrap();

        handle.expect_called(1);
    }

    // RED-PHASE TEST 18: Client::from_config respects timeout and retries.
    #[test]
    fn client_from_config() {
        let config = HttpClientConfig {
            timeout_secs: 10,
            max_retries: 1,
            base_urls: std::collections::HashMap::new(),
        };
        let client = Client::from_config(&config);
        assert_eq!(client.retry_policy.max_retries, 1);
    }

    // RED-PHASE TEST 19: Client.named() preserves mock registry.
    #[test]
    fn named_client_preserves_mock_registry() {
        let registry = Arc::new(MockRegistry::new());
        let client = Client::new().with_mock(registry);
        let named = client.named("stripe");
        assert!(named.mock.is_some());
        assert_eq!(named.alias.as_deref(), Some("stripe"));
    }

    // RED-PHASE TEST 20: base_url is prepended to relative paths.
    #[test]
    fn base_url_prepended_to_relative_path() {
        let client = Client::new();
        let client = client.with_base_url("https://api.stripe.com");
        let builder = client.post("/v1/charges");
        assert_eq!(builder.url, "https://api.stripe.com/v1/charges");
    }

    // RED-PHASE TEST 21: Absolute URLs bypass base_url.
    #[test]
    fn absolute_url_bypasses_base_url() {
        let client = Client::new().with_base_url("https://ignored.example.com");
        let builder = client.get("https://actual.example.com/path");
        assert_eq!(builder.url, "https://actual.example.com/path");
    }

    // RED-PHASE TEST 22: RetryPolicy can be overridden per-request.
    #[test]
    fn retry_override_per_request() {
        let client = Client::new(); // default: 3 retries
        let builder = client.get("https://example.com").retries(0);
        assert_eq!(builder.retry_policy.max_retries, 0);

        let no_retry = client.get("https://example.com").no_retry();
        assert_eq!(no_retry.retry_policy.max_retries, 0);
    }

    // RED-PHASE TEST 23: Client extracts from AppState.
    #[tokio::test]
    async fn client_extracts_from_state() {
        use axum::extract::FromRequestParts;
        let state = crate::AppState::for_test();
        let mut parts = axum::http::Request::new(axum::body::Body::empty())
            .into_parts()
            .0;
        let client = Client::from_request_parts(&mut parts, &state)
            .await
            .unwrap();
        // Default client: no mock, no alias
        assert!(client.mock.is_none());
        assert!(client.alias.is_none());
    }

    // RED-PHASE TEST 24: MockRegistryExt round-trips through AppState extensions.
    #[test]
    fn mock_registry_ext_round_trips_through_state() {
        let registry = Arc::new(MockRegistry::new());
        let ext = HttpMockRegistryExt(registry);
        let state = crate::AppState::for_test();
        state.insert_extension(ext);
        let retrieved = state.extension::<HttpMockRegistryExt>();
        assert!(retrieved.is_some());
    }
}
