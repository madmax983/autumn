//! Authentication utilities for Autumn applications.
//!
//! Provides password hashing, an [`Auth<T>`] extractor for retrieving the
//! authenticated user, and a [`RequireAuth`] middleware layer for protecting
//! routes.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use autumn_web::auth::{Auth, hash_password, verify_password};
//! use autumn_web::session::Session;
//!
//! #[derive(Clone)]
//! struct User { id: i64, name: String }
//!
//! #[post("/register")]
//! async fn register() -> AutumnResult<&'static str> {
//!     let hashed = hash_password("secret123").await?;
//!     // Save hashed password to database...
//!     Ok("registered")
//! }
//!
//! #[post("/login")]
//! async fn login(session: Session) -> AutumnResult<&'static str> {
//!     // Verify credentials...
//!     let stored_hash = "$2b$12$..."; // from database
//!     if verify_password("secret123", stored_hash).await? {
//!         session.insert("user_id", "42").await;
//!         Ok("logged in")
//!     } else {
//!         Err(AutumnError::bad_request_msg("invalid credentials"))
//!     }
//! }
//! ```
//!
//! ## Password hashing
//!
//! Uses bcrypt with a default cost of 12. The [`hash_password`] and
//! [`verify_password`] functions are simple wrappers that return
//! [`AutumnResult`](crate::AutumnResult).
//!
//! ## The `Auth<T>` extractor
//!
//! [`Auth<T>`] extracts the authenticated user from request extensions.
//! It is typically populated by a custom middleware which might call
//! `request.extensions_mut().insert(user)` in a handler. Returns `401 Unauthorized` if no
//! user is present.
//!
//! ## Route protection with `RequireAuth`
//!
//! The [`RequireAuth`] layer rejects unauthenticated requests with
//! `401 Unauthorized` before they reach the handler. It checks for the
//! presence of a session key (default: `"user_id"`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use http::request::Parts;

// ── Password hashing ────────────────────────────────────────────

/// Default bcrypt cost factor.
const DEFAULT_BCRYPT_COST: u32 = 12;

/// Hash a plaintext password using bcrypt.
///
/// Returns the hashed password string suitable for database storage.
///
/// # Errors
///
/// Returns an error if bcrypt hashing fails (extremely unlikely).
///
/// # Examples
///
/// ```rust
/// use autumn_web::auth::hash_password;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let hashed = hash_password("my_secret").await.unwrap();
/// assert!(hashed.starts_with("$2b$"));
/// # });
/// ```
pub async fn hash_password(password: &str) -> crate::AutumnResult<String> {
    let password = password.to_string();
    tokio::task::spawn_blocking(move || {
        bcrypt::hash(password, DEFAULT_BCRYPT_COST)
            .map_err(|e| crate::AutumnError::from(std::io::Error::other(e.to_string())))
    })
    .await
    .map_err(|e| crate::AutumnError::from(std::io::Error::other(e.to_string())))?
}

/// Verify a plaintext password against a bcrypt hash.
///
/// Returns `true` if the password matches the hash.
///
/// # Errors
///
/// Returns an error if bcrypt verification fails (e.g., invalid hash format).
///
/// # Examples
///
/// ```rust
/// use autumn_web::auth::{hash_password, verify_password};
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let hashed = hash_password("my_secret").await.unwrap();
/// assert!(verify_password("my_secret", &hashed).await.unwrap());
/// assert!(!verify_password("wrong_password", &hashed).await.unwrap());
/// # });
/// ```
pub async fn verify_password(password: &str, hash: &str) -> crate::AutumnResult<bool> {
    let password = password.to_string();
    let hash = hash.to_string();

    tokio::task::spawn_blocking(move || {
        // First try to verify.
        bcrypt::verify(&password, &hash).map_or_else(
            |_| {
                // To prevent timing attacks where an invalid hash format returns instantly,
                // we perform a dummy hash calculation so the timing remains roughly the same.
                // We use the same DEFAULT_BCRYPT_COST.
                let _ = bcrypt::hash(&password, DEFAULT_BCRYPT_COST);
                Ok(false)
            },
            Ok,
        )
    })
    .await
    .map_err(|e| crate::AutumnError::from(std::io::Error::other(e.to_string())))?
}

// ── Runtime check for #[secured] macro ──────────────────────────

/// Runtime authentication and authorization check used by the
/// `#[secured]` proc macro. **Not intended for direct use** -- use
/// `#[secured]` instead.
///
/// Checks the session for the configured auth key (default: `"user_id"`).
/// If `roles` is non-empty, also checks that the session's `"role"` value
/// matches at least one of the given roles.
///
/// Returns `401 Unauthorized` if not authenticated, or `403 Forbidden`
/// if the user lacks the required role.
#[doc(hidden)]
pub async fn __check_secured(
    session: &crate::session::Session,
    roles: &[&str],
) -> crate::AutumnResult<()> {
    // Check authentication: session must contain the auth key
    if session.get("user_id").await.is_none() {
        return Err(crate::AutumnError::unauthorized_msg(
            "authentication required",
        ));
    }

    // Check authorization: if roles are specified, the session's "role"
    // must match at least one of them
    if !roles.is_empty() {
        let user_role = session.get("role").await.unwrap_or_default();
        if !roles.iter().any(|&r| r == user_role) {
            return Err(crate::AutumnError::forbidden_msg(
                "insufficient permissions",
            ));
        }
    }

    Ok(())
}

// ── Auth<T> extractor ───────────────────────────────────────────

/// Extractor that retrieves the authenticated user from request extensions.
///
/// Handlers can declare `Auth<MyUser>` as a parameter to access the
/// current user. If no user is present in the request extensions,
/// a `401 Unauthorized` response is returned automatically.
///
/// ## Populating the user
///
/// The user is typically inserted into request extensions by middleware.
/// For example, a custom middleware can load the user from the session
/// and call `request.extensions_mut().insert(user)`.
///
/// ## Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::auth::Auth;
///
/// #[derive(Clone)]
/// struct CurrentUser { id: i64, name: String }
///
/// #[get("/profile")]
/// async fn profile(Auth(user): Auth<CurrentUser>) -> String {
///     format!("Hello, {}!", user.name)
/// }
/// ```
pub struct Auth<T>(pub T);

impl<T, S> FromRequestParts<S> for Auth<T>
where
    T: Clone + Send + Sync + 'static,
    S: Send + Sync,
{
    type Rejection = AuthRejection;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let user = parts.extensions.get::<T>().cloned();
        async move { user.map_or_else(|| Err(AuthRejection), |user| Ok(Self(user))) }
    }
}

/// Rejection type for [`Auth<T>`] when no authenticated user is present.
#[derive(Debug)]
pub struct AuthRejection;

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": {
                    "status": 401,
                    "message": "authentication required"
                }
            })),
        )
            .into_response()
    }
}

impl std::fmt::Display for AuthRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("authentication required")
    }
}

// ── RequireAuth middleware ───────────────────────────────────────

/// Tower [`tower::Layer`] that rejects unauthenticated requests with `401`.
///
/// Checks for a specific key in the session to determine if the request
/// is authenticated. If the key is missing, the request is rejected before
/// reaching the handler.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::auth::RequireAuth;
/// use autumn_web::reexports::axum::{Router, routing::get};
/// use autumn_web::AppState;
///
/// // Protect all routes under /admin
/// let admin_routes = Router::<AppState>::new()
///     .route("/dashboard", get(|| async { "admin" }))
///     .layer(RequireAuth::new("user_id"));
/// ```
#[derive(Clone)]
pub struct RequireAuth {
    session_key: Arc<str>,
}

impl RequireAuth {
    /// Create a new `RequireAuth` layer that checks for the given session key.
    pub fn new(session_key: impl Into<String>) -> Self {
        Self {
            session_key: Arc::from(session_key.into()),
        }
    }
}

impl<S> tower::Layer<S> for RequireAuth {
    type Service = RequireAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequireAuthService {
            inner,
            session_key: Arc::clone(&self.session_key),
        }
    }
}

/// Tower [`tower::Service`] produced by [`RequireAuth`].
#[derive(Clone)]
pub struct RequireAuthService<S> {
    inner: S,
    session_key: Arc<str>,
}

impl<S, ResBody> tower::Service<axum::extract::Request> for RequireAuthService<S>
where
    S: tower::Service<axum::extract::Request, Response = Response<ResBody>>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ResBody: From<String> + Default + Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: axum::extract::Request) -> Self::Future {
        let session_key = Arc::clone(&self.session_key);
        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            // Check if session has the required key
            let session = req.extensions().get::<crate::session::Session>().cloned();

            let is_authenticated = if let Some(ref session) = session {
                session.contains_key(&session_key).await
            } else {
                false
            };

            if is_authenticated {
                inner.call(req).await
            } else {
                let body = serde_json::json!({
                    "error": {
                        "status": 401,
                        "message": "authentication required"
                    }
                });
                let response = Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(ResBody::from(
                        serde_json::to_string(&body).unwrap_or_default(),
                    ))
                    .unwrap_or_default();
                Ok(response)
            }
        })
    }
}

// ── Auth configuration ──────────────────────────────────────────

/// Configuration for authentication.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `bcrypt_cost` | `12` |
/// | `session_key` | `"user_id"` |
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AuthConfig {
    /// Bcrypt cost factor for password hashing.
    #[serde(default = "default_bcrypt_cost")]
    pub bcrypt_cost: u32,

    /// Session key used to identify authenticated users.
    #[serde(default = "default_session_key")]
    pub session_key: String,
}

const fn default_bcrypt_cost() -> u32 {
    DEFAULT_BCRYPT_COST
}

fn default_session_key() -> String {
    "user_id".to_owned()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            bcrypt_cost: default_bcrypt_cost(),
            session_key: default_session_key(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_and_verify_password() {
        let hash = hash_password("test_password").await.unwrap();
        assert!(hash.starts_with("$2b$"));
        assert!(verify_password("test_password", &hash).await.unwrap());
        assert!(!verify_password("wrong_password", &hash).await.unwrap());
    }

    #[tokio::test]
    async fn verify_invalid_hash_returns_false() {
        let result = verify_password("test", "not-a-valid-hash").await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn auth_config_defaults() {
        let config = AuthConfig::default();
        assert_eq!(config.bcrypt_cost, 12);
        assert_eq!(config.session_key, "user_id");
    }

    #[test]
    fn auth_rejection_is_401() {
        let rejection = AuthRejection;
        let response = rejection.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn auth_rejection_display() {
        assert_eq!(AuthRejection.to_string(), "authentication required");
    }

    #[tokio::test]
    async fn auth_extractor_returns_401_when_no_user() {
        use crate::state::AppState;
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;

        #[derive(Clone)]
        struct TestUser {
            name: String,
        }

        async fn handler(Auth(user): Auth<TestUser>) -> String {
            user.name
        }

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let app = Router::new().route("/", get(handler)).with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_extractor_returns_user_when_present() {
        use crate::state::AppState;
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;

        #[derive(Clone)]
        struct TestUser {
            name: String,
        }

        async fn handler(Auth(user): Auth<TestUser>) -> String {
            user.name
        }

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        // Middleware that inserts a user into extensions
        let app = Router::new()
            .route("/", get(handler))
            .layer(axum::middleware::from_fn(
                |mut req: axum::extract::Request, next: axum::middleware::Next| async move {
                    req.extensions_mut().insert(TestUser {
                        name: "alice".into(),
                    });
                    next.run(req).await
                },
            ))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "alice");
    }

    #[tokio::test]
    async fn require_auth_rejects_unauthenticated() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer};
        use crate::state::AppState;

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let app = Router::new()
            .route("/protected", get(|| async { "secret" }))
            .layer(RequireAuth::new("user_id"))
            .layer(SessionLayer::new(
                MemoryStore::new(),
                SessionConfig::default(),
            ))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    // ── __check_secured tests ────────────────────────────────

    #[tokio::test]
    async fn check_secured_rejects_unauthenticated() {
        let session =
            crate::session::Session::new_for_test(String::new(), std::collections::HashMap::new());
        let result = __check_secured(&session, &[]).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn check_secured_allows_authenticated() {
        let data = std::collections::HashMap::from([("user_id".into(), "42".into())]);
        let session = crate::session::Session::new_for_test("sess".into(), data);
        let result = __check_secured(&session, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn check_secured_rejects_wrong_role() {
        let data = std::collections::HashMap::from([
            ("user_id".into(), "42".into()),
            ("role".into(), "viewer".into()),
        ]);
        let session = crate::session::Session::new_for_test("sess".into(), data);
        let result = __check_secured(&session, &["admin"]).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn check_secured_allows_matching_role() {
        let data = std::collections::HashMap::from([
            ("user_id".into(), "42".into()),
            ("role".into(), "admin".into()),
        ]);
        let session = crate::session::Session::new_for_test("sess".into(), data);
        let result = __check_secured(&session, &["admin"]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn check_secured_allows_any_of_multiple_roles() {
        let data = std::collections::HashMap::from([
            ("user_id".into(), "42".into()),
            ("role".into(), "editor".into()),
        ]);
        let session = crate::session::Session::new_for_test("sess".into(), data);
        let result = __check_secured(&session, &["admin", "editor"]).await;
        assert!(result.is_ok());
    }

    // ── #[secured] macro integration tests ──────────────────────

    #[tokio::test]
    async fn secured_macro_rejects_unauthenticated() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer};
        use crate::state::AppState;

        #[autumn_macros::secured]
        async fn protected_handler() -> crate::AutumnResult<&'static str> {
            Ok("secret")
        }

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let app = Router::new()
            .route("/", get(protected_handler))
            .layer(SessionLayer::new(
                MemoryStore::new(),
                SessionConfig::default(),
            ))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn secured_macro_allows_authenticated() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use http::header::COOKIE;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer, SessionStore};
        use crate::state::AppState;

        #[autumn_macros::secured]
        async fn protected_handler() -> crate::AutumnResult<&'static str> {
            Ok("secret")
        }

        let store = MemoryStore::new();
        store
            .save(
                "sess1",
                std::collections::HashMap::from([("user_id".into(), "42".into())]),
            )
            .await
            .unwrap();

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let app = Router::new()
            .route("/", get(protected_handler))
            .layer(SessionLayer::new(store, SessionConfig::default()))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(COOKIE, "autumn.sid=sess1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "secret");
    }

    #[tokio::test]
    async fn secured_macro_with_role_rejects_wrong_role() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use http::header::COOKIE;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer, SessionStore};
        use crate::state::AppState;

        #[autumn_macros::secured("admin")]
        async fn admin_only() -> crate::AutumnResult<&'static str> {
            Ok("admin area")
        }

        let store = MemoryStore::new();
        store
            .save(
                "sess1",
                std::collections::HashMap::from([
                    ("user_id".into(), "42".into()),
                    ("role".into(), "viewer".into()),
                ]),
            )
            .await
            .unwrap();

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let app = Router::new()
            .route("/", get(admin_only))
            .layer(SessionLayer::new(store, SessionConfig::default()))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(COOKIE, "autumn.sid=sess1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn secured_macro_with_multiple_roles_allows_match() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use http::header::COOKIE;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer, SessionStore};
        use crate::state::AppState;

        #[autumn_macros::secured("admin", "editor")]
        async fn content_handler() -> crate::AutumnResult<&'static str> {
            Ok("content")
        }

        let store = MemoryStore::new();
        store
            .save(
                "sess1",
                std::collections::HashMap::from([
                    ("user_id".into(), "42".into()),
                    ("role".into(), "editor".into()),
                ]),
            )
            .await
            .unwrap();

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let app = Router::new()
            .route("/", get(content_handler))
            .layer(SessionLayer::new(store, SessionConfig::default()))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(COOKIE, "autumn.sid=sess1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "content");
    }

    #[tokio::test]
    async fn require_auth_allows_authenticated() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use http::header::COOKIE;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer, SessionStore};
        use crate::state::AppState;

        let store = MemoryStore::new();
        // Pre-populate a session with user_id
        let mut session_data = std::collections::HashMap::new();
        session_data.insert("user_id".into(), "42".into());
        store.save("valid-session", session_data).await.unwrap();

        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let app = Router::new()
            .route("/protected", get(|| async { "secret" }))
            .layer(RequireAuth::new("user_id"))
            .layer(SessionLayer::new(store, SessionConfig::default()))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/protected")
                    .header(COOKIE, "autumn.sid=valid-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "secret");
    }

    #[tokio::test]
    async fn require_auth_poll_ready_propagates() {
        use std::task::{Context, Poll};
        use tower::{Layer, Service};

        #[derive(Clone)]
        struct MockService {
            ready: bool,
        }

        impl Service<axum::extract::Request> for MockService {
            type Response = axum::response::Response;
            type Error = std::convert::Infallible;
            type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                if self.ready {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }

            fn call(&mut self, _req: axum::extract::Request) -> Self::Future {
                std::future::ready(Ok(axum::response::Response::new(axum::body::Body::empty())))
            }
        }

        let layer = RequireAuth::new("user_id");
        let mock_service = MockService { ready: false };
        let mut service = layer.layer(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // When inner is not ready, RequireAuthService should not be ready
        let poll = service.poll_ready(&mut cx);
        assert!(poll.is_pending());

        // When inner is ready, RequireAuthService should be ready
        let mock_service_ready = MockService { ready: true };
        let mut service_ready = layer.layer(mock_service_ready);
        let poll_ready = service_ready.poll_ready(&mut cx);
        assert!(poll_ready.is_ready());
    }
}
