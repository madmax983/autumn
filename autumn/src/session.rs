//! Cookie-based session management for Autumn applications.
//!
//! Provides a [`Session`] extractor that gives handlers access to a
//! per-user key-value store backed by a server-side [`SessionStore`].
//! Session IDs are transmitted via a configurable cookie (default:
//! `autumn.sid`).
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use autumn_web::session::Session;
//!
//! #[get("/dashboard")]
//! async fn dashboard(session: Session) -> AutumnResult<String> {
//!     let user = session.get("user_id").await.unwrap_or_default();
//!     Ok(format!("Hello, {user}"))
//! }
//!
//! #[post("/login")]
//! async fn login(session: Session) -> &'static str {
//!     session.insert("user_id", "alice").await;
//!     "logged in"
//! }
//!
//! #[post("/logout")]
//! async fn logout(session: Session) -> &'static str {
//!     session.clear().await;
//!     "logged out"
//! }
//! ```
//!
//! ## Architecture
//!
//! The session system has three components:
//!
//! 1. **[`SessionStore`]** trait -- pluggable storage backend.
//! 2. **[`MemoryStore`]** -- default in-memory implementation (suitable for
//!    development; data is lost on restart).
//! 3. **[`SessionLayer`]** -- Tower middleware that loads/saves sessions and
//!    manages the session cookie.
//!
//! ## Configuration
//!
//! Configure via `autumn.toml`:
//!
//! ```toml
//! [session]
//! cookie_name = "autumn.sid"
//! max_age_secs = 86400       # 24 hours
//! secure = false              # true in prod profile
//! same_site = "Lax"
//! ```
//!
//! Or via environment variables: `AUTUMN_SESSION__COOKIE_NAME`,
//! `AUTUMN_SESSION__MAX_AGE_SECS`, etc.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{FromRequestParts, Request};
use axum::response::Response;
use http::HeaderValue;
use http::header::{COOKIE, SET_COOKIE};
use http::request::Parts;
use tokio::sync::RwLock;
use tower::{Layer, Service};
use uuid::Uuid;

use crate::AppState;

// ── Session data ────────────────────────────────────────────────

/// A handle to the current request's session data.
///
/// Obtained via the [`Session`] Axum extractor. All reads and writes
/// go through interior mutability so the extractor can be shared.
///
/// Changes are written back to the store automatically when the
/// response is sent (via [`SessionLayer`]).
#[derive(Clone, Debug)]
pub struct Session {
    inner: Arc<RwLock<SessionInner>>,
}

#[derive(Debug)]
struct SessionInner {
    id: String,
    data: HashMap<String, String>,
    dirty: bool,
    destroyed: bool,
}

impl Session {
    /// Create a session for testing purposes.
    #[doc(hidden)]
    #[must_use]
    pub fn new_for_test(id: String, data: HashMap<String, String>) -> Self {
        Self::new(id, data)
    }

    fn new(id: String, data: HashMap<String, String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(SessionInner {
                id,
                data,
                dirty: false,
                destroyed: false,
            })),
        }
    }

    /// Returns the session ID.
    pub async fn id(&self) -> String {
        self.inner.read().await.id.clone()
    }

    /// Get a value from the session.
    pub async fn get(&self, key: &str) -> Option<String> {
        self.inner.read().await.data.get(key).cloned()
    }

    /// Insert a key-value pair into the session.
    pub async fn insert(&self, key: impl Into<String>, value: impl Into<String>) {
        let mut inner = self.inner.write().await;
        inner.data.insert(key.into(), value.into());
        inner.dirty = true;
    }

    /// Remove a key from the session, returning the previous value.
    pub async fn remove(&self, key: &str) -> Option<String> {
        let mut inner = self.inner.write().await;
        let val = inner.data.remove(key);
        if val.is_some() {
            inner.dirty = true;
        }
        val
    }

    /// Remove all data from the session (but keep the session ID).
    pub async fn clear(&self) {
        let mut inner = self.inner.write().await;
        inner.data.clear();
        inner.dirty = true;
    }

    /// Destroy the session entirely. A new session ID will be issued
    /// on the next request.
    pub async fn destroy(&self) {
        let mut inner = self.inner.write().await;
        inner.data.clear();
        inner.destroyed = true;
        inner.dirty = true;
    }

    /// Returns `true` if this session contains the given key.
    pub async fn contains_key(&self, key: &str) -> bool {
        self.inner.read().await.data.contains_key(key)
    }
}

impl FromRequestParts<AppState> for Session {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let session = parts
            .extensions
            .get::<Self>()
            .cloned()
            .expect("SessionLayer must be installed to use the Session extractor");
        async move { Ok(session) }
    }
}

// ── Session store trait ─────────────────────────────────────────

/// Pluggable session storage backend.
///
/// Implement this trait to store sessions in Redis, a database, etc.
/// The default implementation is [`MemoryStore`].
pub trait SessionStore: Send + Sync + 'static {
    /// Load session data for the given ID. Returns `None` if the session
    /// does not exist or has expired.
    fn load(&self, id: &str) -> impl Future<Output = Option<HashMap<String, String>>> + Send;

    /// Save session data under the given ID.
    fn save(&self, id: &str, data: HashMap<String, String>) -> impl Future<Output = ()> + Send;

    /// Delete session data for the given ID.
    fn destroy(&self, id: &str) -> impl Future<Output = ()> + Send;
}

// ── In-memory store ─────────────────────────────────────────────

/// In-memory session store. Suitable for development and testing.
///
/// All data is lost when the process exits. For production use,
/// implement [`SessionStore`] backed by Redis or a database.
#[derive(Clone, Debug, Default)]
pub struct MemoryStore {
    sessions: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
}

impl MemoryStore {
    /// Create a new empty in-memory session store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionStore for MemoryStore {
    async fn load(&self, id: &str) -> Option<HashMap<String, String>> {
        self.sessions.read().await.get(id).cloned()
    }

    async fn save(&self, id: &str, data: HashMap<String, String>) {
        self.sessions.write().await.insert(id.to_owned(), data);
    }

    async fn destroy(&self, id: &str) {
        self.sessions.write().await.remove(id);
    }
}

// ── Session configuration ───────────────────────────────────────

/// Configuration for session management.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `cookie_name` | `"autumn.sid"` |
/// | `max_age_secs` | `86400` (24 hours) |
/// | `secure` | `false` |
/// | `same_site` | `"Lax"` |
/// | `http_only` | `true` |
/// | `path` | `"/"` |
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SessionConfig {
    /// Name of the session cookie.
    #[serde(default = "default_cookie_name")]
    pub cookie_name: String,

    /// Maximum age of the session cookie in seconds.
    #[serde(default = "default_max_age_secs")]
    pub max_age_secs: u64,

    /// Whether the cookie should only be sent over HTTPS.
    #[serde(default)]
    pub secure: bool,

    /// `SameSite` attribute for the cookie.
    #[serde(default = "default_same_site")]
    pub same_site: String,

    /// Whether the cookie should be inaccessible to JavaScript.
    #[serde(default = "default_true")]
    pub http_only: bool,

    /// Path scope for the cookie.
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_cookie_name() -> String {
    "autumn.sid".to_owned()
}
const fn default_max_age_secs() -> u64 {
    86400
}
fn default_same_site() -> String {
    "Lax".to_owned()
}
const fn default_true() -> bool {
    true
}
fn default_path() -> String {
    "/".to_owned()
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cookie_name: default_cookie_name(),
            max_age_secs: default_max_age_secs(),
            secure: false,
            same_site: default_same_site(),
            http_only: default_true(),
            path: default_path(),
        }
    }
}

// ── Cookie helpers ──────────────────────────────────────────────

/// Extract a named cookie value from the Cookie header.
fn get_cookie(headers: &http::HeaderMap, name: &str) -> Option<String> {
    headers.get_all(COOKIE).iter().find_map(|value| {
        value.to_str().ok().and_then(|s| {
            s.split(';').map(str::trim).find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                if k.trim() == name {
                    Some(v.trim().to_owned())
                } else {
                    None
                }
            })
        })
    })
}

/// Build a Set-Cookie header value.
fn build_set_cookie(config: &SessionConfig, session_id: &str) -> String {
    use std::fmt::Write;
    let mut cookie = format!(
        "{}={}; Path={}",
        config.cookie_name, session_id, config.path
    );
    let _ = write!(cookie, "; Max-Age={}", config.max_age_secs);
    if config.http_only {
        cookie.push_str("; HttpOnly");
    }
    if config.secure {
        cookie.push_str("; Secure");
    }
    let _ = write!(cookie, "; SameSite={}", config.same_site);
    cookie
}

/// Build a Set-Cookie header that expires the cookie immediately.
fn build_expire_cookie(config: &SessionConfig) -> String {
    format!(
        "{}=; Path={}; Max-Age=0; HttpOnly; SameSite={}",
        config.cookie_name, config.path, config.same_site
    )
}

// ── Session layer (Tower middleware) ────────────────────────────

/// Tower [`Layer`] that manages session loading, saving, and cookie handling.
///
/// Install this on the Autumn router to enable the [`Session`] extractor.
/// The framework installs it automatically when a session store is configured.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::session::{SessionLayer, MemoryStore, SessionConfig};
///
/// let layer = SessionLayer::new(MemoryStore::new(), SessionConfig::default());
/// ```
#[derive(Clone)]
pub struct SessionLayer<S: SessionStore> {
    store: Arc<S>,
    config: Arc<SessionConfig>,
}

impl<S: SessionStore> SessionLayer<S> {
    /// Create a new session layer with the given store and configuration.
    pub fn new(store: S, config: SessionConfig) -> Self {
        Self {
            store: Arc::new(store),
            config: Arc::new(config),
        }
    }
}

impl<S: SessionStore + Clone, Inner> Layer<Inner> for SessionLayer<S> {
    type Service = SessionService<S, Inner>;

    fn layer(&self, inner: Inner) -> Self::Service {
        SessionService {
            inner,
            store: Arc::clone(&self.store),
            config: Arc::clone(&self.config),
        }
    }
}

/// Tower [`Service`] produced by [`SessionLayer`].
#[derive(Clone)]
pub struct SessionService<S: SessionStore, Inner> {
    inner: Inner,
    store: Arc<S>,
    config: Arc<SessionConfig>,
}

impl<St, Inner, ResBody> Service<Request> for SessionService<St, Inner>
where
    St: SessionStore + Clone,
    Inner: Service<Request, Response = Response<ResBody>> + Clone + Send + 'static,
    Inner::Future: Send + 'static,
    Inner::Error: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = Inner::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let store = Arc::clone(&self.store);
        let config = Arc::clone(&self.config);
        let mut inner = self.inner.clone();
        // Swap to ensure correct poll_ready semantics
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            // 1. Extract or create session ID
            let existing_id = get_cookie(req.headers(), &config.cookie_name);
            let (session_id, data) = if let Some(ref id) = existing_id {
                let data = store.load(id).await.unwrap_or_default();
                (id.clone(), data)
            } else {
                (Uuid::new_v4().to_string(), HashMap::new())
            };

            // 2. Create session handle and insert into extensions
            let session = Session::new(session_id.clone(), data);
            req.extensions_mut().insert(session.clone());

            // 3. Call inner service
            let mut response = inner.call(req).await?;

            // 4. Save or destroy session based on state
            let inner_guard = session.inner.read().await;
            if inner_guard.destroyed {
                store.destroy(&session_id).await;
                if let Ok(val) = HeaderValue::from_str(&build_expire_cookie(&config)) {
                    response.headers_mut().append(SET_COOKIE, val);
                }
            } else if inner_guard.dirty || existing_id.is_none() {
                let data = inner_guard.data.clone();
                let sid = inner_guard.id.clone();
                drop(inner_guard);
                store.save(&sid, data).await;
                if let Ok(val) = HeaderValue::from_str(&build_set_cookie(&config, &sid)) {
                    response.headers_mut().append(SET_COOKIE, val);
                }
            }

            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use http::Request as HttpRequest;
    use tower::ServiceExt;

    #[tokio::test]
    async fn memory_store_save_and_load() {
        let store = MemoryStore::new();
        let mut data = HashMap::new();
        data.insert("user".into(), "alice".into());
        store.save("sess1", data).await;

        let loaded = store.load("sess1").await;
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().get("user").unwrap(), "alice");
    }

    #[tokio::test]
    async fn memory_store_destroy() {
        let store = MemoryStore::new();
        store.save("sess1", HashMap::new()).await;
        store.destroy("sess1").await;
        assert!(store.load("sess1").await.is_none());
    }

    #[tokio::test]
    async fn memory_store_load_missing() {
        let store = MemoryStore::new();
        assert!(store.load("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn session_insert_and_get() {
        let session = Session::new("test".into(), HashMap::new());
        session.insert("key", "value").await;
        assert_eq!(session.get("key").await, Some("value".to_owned()));
    }

    #[tokio::test]
    async fn session_remove() {
        let mut data = HashMap::new();
        data.insert("key".into(), "value".into());
        let session = Session::new("test".into(), data);
        let removed = session.remove("key").await;
        assert_eq!(removed, Some("value".to_owned()));
        assert!(session.get("key").await.is_none());
    }

    #[tokio::test]
    async fn session_clear() {
        let mut data = HashMap::new();
        data.insert("a".into(), "1".into());
        data.insert("b".into(), "2".into());
        let session = Session::new("test".into(), data);
        session.clear().await;
        assert!(session.get("a").await.is_none());
        assert!(session.get("b").await.is_none());
    }

    #[tokio::test]
    async fn session_contains_key() {
        let mut data = HashMap::new();
        data.insert("exists".into(), "yes".into());
        let session = Session::new("test".into(), data);
        assert!(session.contains_key("exists").await);
        assert!(!session.contains_key("missing").await);
    }

    #[tokio::test]
    async fn session_destroy_marks_destroyed() {
        let session = Session::new("test".into(), HashMap::new());
        session.insert("key", "value").await;
        session.destroy().await;
        let inner = session.inner.read().await;
        let destroyed = inner.destroyed;
        let empty = inner.data.is_empty();
        drop(inner);
        assert!(destroyed);
        assert!(empty);
    }

    #[test]
    fn get_cookie_extracts_value() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("autumn.sid=abc123; other=xyz"),
        );
        assert_eq!(get_cookie(&headers, "autumn.sid"), Some("abc123".into()));
        assert_eq!(get_cookie(&headers, "other"), Some("xyz".into()));
        assert_eq!(get_cookie(&headers, "missing"), None);
    }

    #[test]
    fn build_set_cookie_contains_required_parts() {
        let config = SessionConfig::default();
        let cookie = build_set_cookie(&config, "test-id");
        assert!(cookie.contains("autumn.sid=test-id"));
        assert!(cookie.contains("Path=/"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Max-Age=86400"));
    }

    #[test]
    fn build_expire_cookie_has_zero_max_age() {
        let config = SessionConfig::default();
        let cookie = build_expire_cookie(&config);
        assert!(cookie.contains("Max-Age=0"));
    }

    #[test]
    fn session_config_defaults() {
        let config = SessionConfig::default();
        assert_eq!(config.cookie_name, "autumn.sid");
        assert_eq!(config.max_age_secs, 86400);
        assert!(!config.secure);
        assert_eq!(config.same_site, "Lax");
        assert!(config.http_only);
        assert_eq!(config.path, "/");
    }

    #[tokio::test]
    async fn session_layer_sets_cookie_on_new_session() {
        async fn handler(session: Session) -> String {
            session.insert("visited", "true").await;
            "ok".to_owned()
        }

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
        };

        let app = Router::new()
            .route("/", get(handler))
            .layer(SessionLayer::new(
                MemoryStore::new(),
                SessionConfig::default(),
            ))
            .with_state(state);

        let response = app
            .oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), http::StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("should set session cookie");
        let cookie_str = set_cookie.to_str().unwrap();
        assert!(cookie_str.contains("autumn.sid="));
    }

    #[tokio::test]
    async fn session_layer_persists_data_across_requests() {
        async fn write_handler(session: Session) -> String {
            session.insert("user", "alice").await;
            "saved".to_owned()
        }

        async fn read_handler(session: Session) -> String {
            session.get("user").await.unwrap_or_default()
        }

        let store = MemoryStore::new();
        let config = SessionConfig::default();
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
        };

        let app = Router::new()
            .route("/write", get(write_handler))
            .route("/read", get(read_handler))
            .layer(SessionLayer::new(store, config))
            .with_state(state);

        // First request: write to session
        let resp1 = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/write")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let cookie = resp1
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        // Extract just the cookie value for the next request
        let session_cookie = cookie.split(';').next().unwrap();

        // Second request: read from session
        let resp2 = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/read")
                    .header(COOKIE, session_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "alice");
    }

    #[tokio::test]
    async fn session_destroy_expires_cookie() {
        async fn handler(session: Session) -> String {
            session.destroy().await;
            "destroyed".to_owned()
        }

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
        };

        let store = MemoryStore::new();
        store
            .save("existing-id", HashMap::from([("k".into(), "v".into())]))
            .await;

        let app = Router::new()
            .route("/", get(handler))
            .layer(SessionLayer::new(store.clone(), SessionConfig::default()))
            .with_state(state);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .header(COOKIE, "autumn.sid=existing-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("Max-Age=0"), "cookie should be expired");

        // Store should no longer have the session
        assert!(store.load("existing-id").await.is_none());
    }
}
