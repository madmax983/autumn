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

#[cfg(feature = "oauth2")]
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
#[cfg(feature = "oauth2")]
use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use http::request::Parts;
#[cfg(feature = "oauth2")]
use jsonwebtoken::jwk::JwkSet;
#[cfg(feature = "oauth2")]
use serde::Deserialize;
#[cfg(feature = "oauth2")]
use url::Url;

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

    // Parse the hash format outside the blocking task.
    // A valid bcrypt hash is typically 60 characters and starts with "$".
    let is_valid_format = hash.len() == 60 && hash.starts_with('$');

    let hash_to_verify = if is_valid_format {
        hash.to_string()
    } else {
        // To prevent timing attacks, perform a dummy verification against a known hash.
        "$2b$12$KIXe8K4j1sH6/xH.x9d71uJ5Jk8t6O4m6Q110g4H8y1r6J6O6O6O6".to_string()
    };

    let result = tokio::task::spawn_blocking(move || bcrypt::verify(&password, &hash_to_verify))
        .await
        .map_err(|e| crate::AutumnError::from(std::io::Error::other(e.to_string())))?;

    if !is_valid_format {
        return Ok(false);
    }

    result.map_err(|e| crate::AutumnError::from(std::io::Error::other(e.to_string())))
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
    __check_secured_with_key(session, "user_id", roles).await
}

/// Runtime check used by `#[secured]` when `AppState` is available.
///
/// Accepts the configured auth session key so generated login/signup/reset
/// handlers and `#[secured]` resolve authentication through the same session
/// entry.
#[doc(hidden)]
pub async fn __check_secured_with_key(
    session: &crate::session::Session,
    auth_session_key: &str,
    roles: &[&str],
) -> crate::AutumnResult<()> {
    // Check authentication: session must contain the auth key
    if session.get(auth_session_key).await.is_none() {
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
        crate::AutumnError::unauthorized_msg("authentication required").into_response()
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
                let body = crate::error::problem_details_json_string(
                    StatusCode::UNAUTHORIZED,
                    "authentication required",
                    None,
                    None,
                    req.extensions()
                        .get::<crate::middleware::RequestId>()
                        .map(std::string::ToString::to_string),
                    Some(req.uri().path().to_owned()),
                    true,
                );
                let response = Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(http::header::CONTENT_TYPE, "application/problem+json")
                    .body(ResBody::from(body))
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

    /// OAuth2/OIDC provider configuration by provider key
    /// (for example: `github`, `google`, `okta`).
    #[cfg(feature = "oauth2")]
    #[serde(default)]
    pub oauth2: OAuth2Config,
}

const fn default_bcrypt_cost() -> u32 {
    DEFAULT_BCRYPT_COST
}

fn default_session_key() -> String {
    "user_id".to_owned()
}

#[cfg(feature = "oauth2")]
const fn default_provider_scope() -> String {
    String::new()
}

#[cfg(feature = "oauth2")]
const OAUTH_HTTP_TIMEOUT_SECS: u64 = 15;

#[cfg(feature = "oauth2")]
/// `OAuth2` provider map loaded from `autumn.toml`.
///
/// Example:
///
/// ```toml
/// [auth.oauth2.github]
/// client_id = "..."
/// client_secret = "..."
/// authorize_url = "https://github.com/login/oauth/authorize"
/// token_url = "https://github.com/login/oauth/access_token"
/// userinfo_url = "https://api.github.com/user"
/// redirect_uri = "http://localhost:3000/auth/github/callback"
/// scope = "read:user user:email"
/// ```
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct OAuth2Config {
    /// Dynamic provider table keyed by provider name.
    #[serde(flatten)]
    pub providers: HashMap<String, OAuth2ProviderConfig>,
}

#[cfg(feature = "oauth2")]
/// A single OAuth2/OIDC provider configuration entry.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OAuth2ProviderConfig {
    /// The client ID provided by the `OAuth2` identity provider.
    pub client_id: String,
    /// The client secret provided by the `OAuth2` identity provider.
    pub client_secret: String,
    /// The authorization endpoint URL where users are redirected to authenticate.
    pub authorize_url: String,
    /// The token endpoint URL used to exchange an authorization code for tokens.
    pub token_url: String,
    /// The optional userinfo endpoint URL used to fetch profile details.
    #[serde(default)]
    pub userinfo_url: Option<String>,
    /// The local redirect URI registered with the identity provider (e.g., `http://localhost/auth/callback`).
    pub redirect_uri: String,
    /// The requested scope string (e.g., `openid profile email`).
    #[serde(default = "default_provider_scope")]
    pub scope: String,
    /// Expected OIDC issuer (`iss`) used for ID token validation.
    #[serde(default)]
    pub issuer: Option<String>,
    /// JWKS endpoint URL used to verify ID token signatures.
    #[serde(default)]
    pub jwks_url: Option<String>,
}

#[cfg(feature = "oauth2")]
/// Query extractor payload for `OAuth2` callback handlers.
#[derive(Debug, Clone, Deserialize)]
pub struct OAuth2Callback {
    /// The authorization code returned by the provider.
    pub code: String,
    /// The anti-CSRF state token passed during the authorization request.
    pub state: String,
}

#[cfg(feature = "oauth2")]
/// Identity information extracted from an OIDC ID token or userinfo endpoint.
#[derive(Debug, Clone)]
pub struct OidcIdentity {
    /// The primary subject identifier (`sub` claim) representing the user.
    pub subject: String,
    /// The user's email address, if available in the claims.
    pub email: Option<String>,
    /// The user's full name, if available in the claims.
    pub name: Option<String>,
    /// The user's preferred username or nickname, if available in the claims.
    pub preferred_username: Option<String>,
    /// The raw JSON claims extracted from the token or userinfo response.
    pub raw_claims: serde_json::Value,
}

#[cfg(feature = "oauth2")]
#[derive(Debug, Deserialize)]
struct OAuth2TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: Option<String>,
    id_token: Option<String>,
}

#[cfg(feature = "oauth2")]
/// Build an `OAuth2` authorization URL and persist anti-CSRF state + nonce in session.
///
/// # Errors
///
/// Returns an error if `authorize_url` is not a valid URL.
pub async fn oauth2_authorize_url(
    session: &crate::session::Session,
    provider_name: &str,
    provider: &OAuth2ProviderConfig,
) -> crate::AutumnResult<String> {
    let state = uuid::Uuid::new_v4().to_string();
    let nonce = uuid::Uuid::new_v4().to_string();
    session
        .insert(format!("oauth2:{provider_name}:state"), state.clone())
        .await;
    session
        .insert(format!("oauth2:{provider_name}:nonce"), nonce.clone())
        .await;

    let mut url = Url::parse(&provider.authorize_url)
        .map_err(|e| crate::AutumnError::bad_request_msg(format!("invalid authorize_url: {e}")))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", &provider.client_id);
        q.append_pair("redirect_uri", &provider.redirect_uri);
        if !provider.scope.trim().is_empty() {
            q.append_pair("scope", &provider.scope);
        }
        q.append_pair("state", &state);
        q.append_pair("nonce", &nonce);
    }
    Ok(url.into())
}

#[cfg(feature = "oauth2")]
/// Exchange callback code for tokens, validate state/nonce, and return OIDC identity.
///
/// On success this method rotates the session ID and writes:
/// - `session_key` (OIDC `sub`)
/// - `auth_provider` (provider key, like `github`)
///
/// # Errors
///
/// Returns an error when callback state/nonce validation fails, token exchange
/// fails, ID token/userinfo payloads are invalid, or identity extraction fails.
pub async fn oauth2_finish_login(
    session: &crate::session::Session,
    session_key: &str,
    provider_name: &str,
    provider: &OAuth2ProviderConfig,
    callback: &OAuth2Callback,
) -> crate::AutumnResult<OidcIdentity> {
    validate_callback_state(session, provider_name, callback).await?;
    let token = exchange_oauth2_token(provider, callback).await?;
    let (claims, source) = load_identity_claims(provider, &token).await?;
    validate_oidc_nonce(session, provider_name, &claims, source).await?;
    let subject = extract_subject(&claims, source)?;
    finalize_oauth2_session(session, session_key, provider_name, subject, claims).await
}

#[cfg(feature = "oauth2")]
async fn validate_callback_state(
    session: &crate::session::Session,
    provider_name: &str,
    callback: &OAuth2Callback,
) -> crate::AutumnResult<()> {
    let state_key = format!("oauth2:{provider_name}:state");
    // Read without removing so a stray/attacker-controlled callback with a
    // wrong state value cannot consume the real state and break the pending
    // legitimate redirect.
    let expected_state = session.get(&state_key).await.ok_or_else(|| {
        crate::AutumnError::unauthorized_msg("oauth2 state missing; restart login")
    })?;
    if subtle::ConstantTimeEq::ct_eq(expected_state.as_bytes(), callback.state.as_bytes())
        .unwrap_u8()
        != 1
    {
        return Err(crate::AutumnError::unauthorized_msg(
            "oauth2 state mismatch",
        ));
    }
    // Remove the state only after a successful constant-time match.
    session.remove(&state_key).await;
    Ok(())
}

#[cfg(feature = "oauth2")]
async fn exchange_oauth2_token(
    provider: &OAuth2ProviderConfig,
    callback: &OAuth2Callback,
) -> crate::AutumnResult<OAuth2TokenResponse> {
    let token_response = oauth_http_client()?
        .post(&provider.token_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", callback.code.as_str()),
            ("redirect_uri", provider.redirect_uri.as_str()),
            ("client_id", provider.client_id.as_str()),
            ("client_secret", provider.client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(|e| {
            crate::AutumnError::service_unavailable_msg(format!("token request failed: {e}"))
        })?
        .error_for_status()
        .map_err(|e| crate::AutumnError::unauthorized_msg(format!("token exchange failed: {e}")))?;

    let token_content_type = token_response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let token_body = token_response.text().await.map_err(|e| {
        crate::AutumnError::bad_request_msg(format!("invalid token response body: {e}"))
    })?;
    parse_oauth2_token_response(token_content_type.as_deref(), &token_body)
}

#[cfg(feature = "oauth2")]
async fn load_identity_claims(
    provider: &OAuth2ProviderConfig,
    token: &OAuth2TokenResponse,
) -> crate::AutumnResult<(serde_json::Value, IdentitySource)> {
    if let Some(id_token) = token.id_token.as_deref() {
        return Ok((
            validate_and_decode_id_token(id_token, provider).await?,
            IdentitySource::IdToken,
        ));
    }
    if let Some(userinfo_url) = &provider.userinfo_url {
        let claims = oauth_http_client()?
            .get(userinfo_url)
            .header(
                reqwest::header::USER_AGENT,
                concat!("autumn-web/", env!("CARGO_PKG_VERSION")),
            )
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(|e| {
                crate::AutumnError::service_unavailable_msg(format!("userinfo request failed: {e}"))
            })?
            .error_for_status()
            .map_err(|e| crate::AutumnError::unauthorized_msg(format!("userinfo failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                crate::AutumnError::bad_request_msg(format!("invalid userinfo payload: {e}"))
            })?;
        return Ok((claims, IdentitySource::UserInfo));
    }
    Err(crate::AutumnError::bad_request_msg(
        "provider must return id_token or configure userinfo_url",
    ))
}

#[cfg(feature = "oauth2")]
async fn validate_oidc_nonce(
    session: &crate::session::Session,
    provider_name: &str,
    claims: &serde_json::Value,
    source: IdentitySource,
) -> crate::AutumnResult<()> {
    let nonce_key = format!("oauth2:{provider_name}:nonce");
    let stored_nonce = session.remove(&nonce_key).await;
    if source == IdentitySource::IdToken {
        // The nonce MUST be present in the session for ID-token logins.
        // A missing nonce (e.g. session was partially cleared) must be
        // treated as an error to prevent replay/mix-up attacks.
        let expected_nonce = stored_nonce.ok_or_else(|| {
            crate::AutumnError::unauthorized_msg("oauth2 nonce missing from session")
        })?;
        let actual_nonce = claims
            .get("nonce")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| crate::AutumnError::unauthorized_msg("missing oidc nonce claim"))?;
        if subtle::ConstantTimeEq::ct_eq(expected_nonce.as_bytes(), actual_nonce.as_bytes())
            .unwrap_u8()
            != 1
        {
            return Err(crate::AutumnError::unauthorized_msg("oidc nonce mismatch"));
        }
    }
    Ok(())
}

#[cfg(feature = "oauth2")]
async fn finalize_oauth2_session(
    session: &crate::session::Session,
    session_key: &str,
    provider_name: &str,
    subject: String,
    claims: serde_json::Value,
) -> crate::AutumnResult<OidcIdentity> {
    session.insert(session_key, subject.clone()).await;
    session.insert("auth_provider", provider_name).await;
    session.rotate_id().await;
    Ok(OidcIdentity {
        subject,
        email: claims
            .get("email")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        name: claims
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        preferred_username: claims
            .get("preferred_username")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        raw_claims: claims,
    })
}

#[cfg(feature = "oauth2")]
fn parse_oauth2_token_response(
    content_type: Option<&str>,
    body: &str,
) -> crate::AutumnResult<OAuth2TokenResponse> {
    let looks_like_json = content_type.is_some_and(|v| v.contains("application/json"))
        || body.trim_start().starts_with('{');
    if looks_like_json {
        return serde_json::from_str(body).map_err(|e| {
            crate::AutumnError::bad_request_msg(format!("invalid json token response: {e}"))
        });
    }

    let mut access_token = None;
    let mut token_type = None;
    let mut id_token = None;

    for (k, v) in url::form_urlencoded::parse(body.as_bytes()) {
        match k.as_ref() {
            "access_token" => access_token = Some(v.into_owned()),
            "token_type" => token_type = Some(v.into_owned()),
            "id_token" => id_token = Some(v.into_owned()),
            _ => {}
        }
    }

    let access_token = access_token.ok_or_else(|| {
        crate::AutumnError::bad_request_msg("token response missing access_token")
    })?;

    Ok(OAuth2TokenResponse {
        access_token,
        token_type,
        id_token,
    })
}

#[cfg(feature = "oauth2")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentitySource {
    IdToken,
    UserInfo,
}

#[cfg(feature = "oauth2")]
fn extract_subject(
    claims: &serde_json::Value,
    source: IdentitySource,
) -> crate::AutumnResult<String> {
    if let Some(sub) = claims.get("sub").and_then(serde_json::Value::as_str) {
        return Ok(sub.to_owned());
    }

    if source == IdentitySource::UserInfo {
        if let Some(id) = claims.get("id").and_then(serde_json::Value::as_i64) {
            return Ok(id.to_string());
        }
        if let Some(id) = claims.get("id").and_then(serde_json::Value::as_str) {
            return Ok(id.to_owned());
        }
        return Err(crate::AutumnError::bad_request_msg(
            "missing identity claim: expected sub or id from userinfo",
        ));
    }

    Err(crate::AutumnError::bad_request_msg("missing sub claim"))
}

#[cfg(feature = "oauth2")]
async fn validate_and_decode_id_token(
    token: &str,
    provider: &OAuth2ProviderConfig,
) -> crate::AutumnResult<serde_json::Value> {
    let issuer = provider
        .issuer
        .as_deref()
        .ok_or_else(|| crate::AutumnError::bad_request_msg("provider.issuer required for oidc"))?;
    let jwks_url = provider.jwks_url.as_deref().ok_or_else(|| {
        crate::AutumnError::bad_request_msg("provider.jwks_url required for oidc")
    })?;

    let header = jsonwebtoken::decode_header(token).map_err(|e| {
        crate::AutumnError::unauthorized_msg(format!("invalid id_token header: {e}"))
    })?;
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| crate::AutumnError::unauthorized_msg("id_token header missing kid"))?;
    let alg = header.alg;

    let jwks: JwkSet = oauth_http_client()?
        .get(jwks_url)
        .send()
        .await
        .map_err(|e| {
            crate::AutumnError::service_unavailable_msg(format!("jwks request failed: {e}"))
        })?
        .error_for_status()
        .map_err(|e| crate::AutumnError::unauthorized_msg(format!("jwks fetch failed: {e}")))?
        .json()
        .await
        .map_err(|e| crate::AutumnError::bad_request_msg(format!("invalid jwks response: {e}")))?;

    let jwk = jwks
        .keys
        .iter()
        .find(|k| k.common.key_id.as_deref() == Some(kid))
        .ok_or_else(|| crate::AutumnError::unauthorized_msg("no jwk matched id_token kid"))?;
    let decoding_key = jsonwebtoken::DecodingKey::from_jwk(jwk)
        .map_err(|e| crate::AutumnError::unauthorized_msg(format!("invalid jwk key: {e}")))?;

    let mut validation = jsonwebtoken::Validation::new(alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(std::slice::from_ref(&provider.client_id));
    validation.required_spec_claims = ["exp", "iss", "aud", "sub"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    validation.validate_exp = true;
    validation.validate_nbf = true;

    let claims = jsonwebtoken::decode::<serde_json::Value>(token, &decoding_key, &validation)
        .map_err(|e| crate::AutumnError::unauthorized_msg(format!("invalid id_token: {e}")))?;
    Ok(claims.claims)
}

#[cfg(feature = "oauth2")]
fn oauth_http_client() -> crate::AutumnResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(OAUTH_HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| {
            crate::AutumnError::service_unavailable_msg(format!(
                "failed to build oauth http client: {e}"
            ))
        })
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            bcrypt_cost: default_bcrypt_cost(),
            session_key: default_session_key(),
            #[cfg(feature = "oauth2")]
            oauth2: OAuth2Config::default(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API Token Authentication
// ─────────────────────────────────────────────────────────────────────────────

/// Backend trait for storing and verifying API bearer tokens.
///
/// Implementations persist only the token hash — the raw token is never stored
/// at rest. The default backend for tests is [`InMemoryApiTokenStore`].
/// Production deployments should use a database-backed implementation.
///
/// All methods take `&self`; use interior mutability where write access is
/// needed.
pub trait ApiTokenStore: Send + Sync + 'static {
    /// Issue a new token for `principal_id` and return the raw value.
    ///
    /// Only the hash is persisted. The raw token must be delivered to the
    /// caller immediately — it cannot be recovered later.
    fn issue<'a>(
        &'a self,
        principal_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<String>> + Send + 'a>>;

    /// Verify `raw_token` and return its principal ID, or `None` for unknown
    /// or revoked tokens.
    fn verify<'a>(
        &'a self,
        raw_token: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<Option<String>>> + Send + 'a>>;

    /// Revoke a token so that subsequent requests are rejected.
    fn revoke<'a>(
        &'a self,
        raw_token: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<()>> + Send + 'a>>;
}

/// Compute the SHA-256 hash of a raw API token as a lowercase 64-char hex string.
///
/// The hash is deterministic: the same input always produces the same output.
/// Only the hash is ever stored; the raw token is never persisted.
///
/// # Examples
///
/// ```rust
/// use autumn_web::auth::hash_api_token;
///
/// let h = hash_api_token("my_token");
/// assert_eq!(h.len(), 64);
/// assert_eq!(h, hash_api_token("my_token")); // deterministic
/// ```
#[must_use]
pub fn hash_api_token(raw: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(raw.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Generate a 256-bit random raw API token as a lowercase hex string.
///
/// Uses two UUID v4 values (128 bits each) concatenated for a 64-char result.
fn generate_raw_token() -> String {
    let u1 = uuid::Uuid::new_v4();
    let u2 = uuid::Uuid::new_v4();
    format!("{}{}", u1.simple(), u2.simple())
}

/// In-memory API token store for development and testing.
///
/// Tokens are stored as SHA-256 hashes mapped to principal IDs inside a
/// `RwLock`-protected `HashMap`. **Not suitable for production** — state is
/// lost on restart and is not shared across processes.
///
/// # Examples
///
/// ```rust
/// use std::sync::Arc;
/// use autumn_web::auth::{ApiTokenStore, InMemoryApiTokenStore};
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let store = Arc::new(InMemoryApiTokenStore::default());
/// let token = store.issue("user:1").await.unwrap();
/// assert_eq!(store.verify(&token).await.unwrap(), Some("user:1".to_owned()));
/// store.revoke(&token).await.unwrap();
/// assert_eq!(store.verify(&token).await.unwrap(), None);
/// # });
/// ```
#[derive(Clone)]
pub struct InMemoryApiTokenStore {
    // hash → principal_id
    tokens: Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>,
}

impl Default for InMemoryApiTokenStore {
    fn default() -> Self {
        Self {
            tokens: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl ApiTokenStore for InMemoryApiTokenStore {
    fn issue<'a>(
        &'a self,
        principal_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<String>> + Send + 'a>> {
        Box::pin(async move {
            let raw = generate_raw_token();
            let hash = hash_api_token(&raw);
            self.tokens
                .write()
                .expect("api token store lock poisoned")
                .insert(hash, principal_id.to_owned());
            Ok(raw)
        })
    }

    fn verify<'a>(
        &'a self,
        raw_token: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<Option<String>>> + Send + 'a>> {
        Box::pin(async move {
            let hash = hash_api_token(raw_token);
            Ok(self
                .tokens
                .read()
                .expect("api token store lock poisoned")
                .get(&hash)
                .cloned())
        })
    }

    fn revoke<'a>(
        &'a self,
        raw_token: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let hash = hash_api_token(raw_token);
            self.tokens
                .write()
                .expect("api token store lock poisoned")
                .remove(&hash);
            Ok(())
        })
    }
}

/// Issue a new API token for `principal_id` using `store`.
///
/// Returns the raw token string that must be transmitted to the client once.
///
/// # Errors
///
/// Propagates any error from the underlying store.
pub async fn issue_api_token(
    store: &dyn ApiTokenStore,
    principal_id: &str,
) -> crate::AutumnResult<String> {
    store.issue(principal_id).await
}

/// Revoke a previously issued API token using `store`.
///
/// After revocation [`RequireApiToken`] rejects requests presenting this token.
///
/// # Errors
///
/// Propagates any error from the underlying store.
pub async fn revoke_api_token(
    store: &dyn ApiTokenStore,
    raw_token: &str,
) -> crate::AutumnResult<()> {
    store.revoke(raw_token).await
}

/// Private marker inserted into request extensions by [`RequireApiToken`] after
/// a bearer token is successfully verified.
#[derive(Clone)]
struct ApiTokenPrincipal(String);

/// Extractor that yields the verified principal ID from a bearer-protected route.
///
/// The principal ID is inserted by [`RequireApiToken`] after verifying the
/// `Authorization: Bearer <token>` header. Without `RequireApiToken` on the
/// route this extractor returns `401 Unauthorized`.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::auth::ApiToken;
///
/// #[get("/whoami")]
/// async fn whoami(ApiToken(principal): ApiToken) -> String {
///     format!("authenticated as {principal}")
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ApiToken(pub String);

impl<S> FromRequestParts<S> for ApiToken
where
    S: Send + Sync,
{
    type Rejection = AuthRejection;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let principal = parts.extensions.get::<ApiTokenPrincipal>().cloned();
        async move { principal.map(|p| Self(p.0)).ok_or(AuthRejection) }
    }
}

/// Tower [`Layer`](tower::Layer) that validates `Authorization: Bearer <token>`
/// on every inbound request.
///
/// On success the verified principal ID is inserted into request extensions
/// so handlers can retrieve it via the [`ApiToken`] extractor.
/// Requests with a missing, malformed, or revoked token are rejected with
/// `401 Unauthorized` using the same Problem Details contract as
/// [`AuthRejection`].
///
/// Composes with [`RequireAuth`] and session middleware without conflict.
///
/// # Examples
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use autumn_web::auth::{InMemoryApiTokenStore, RequireApiToken};
/// use autumn_web::reexports::axum::{Router, routing::get};
/// use autumn_web::AppState;
///
/// let store = Arc::new(InMemoryApiTokenStore::default());
/// let api_routes = Router::<AppState>::new()
///     .route("/data", get(|| async { "ok" }))
///     .layer(RequireApiToken::new(store));
/// ```
#[derive(Clone)]
pub struct RequireApiToken {
    store: Arc<dyn ApiTokenStore>,
}

impl RequireApiToken {
    /// Create a new [`RequireApiToken`] layer backed by `store`.
    ///
    /// Accepts any `Arc<S>` where `S: ApiTokenStore`, so callers do not need
    /// to explicitly cast to `Arc<dyn ApiTokenStore>`.
    #[must_use]
    pub fn new<S: ApiTokenStore + 'static>(store: Arc<S>) -> Self {
        Self { store }
    }
}

impl<S> tower::Layer<S> for RequireApiToken {
    type Service = RequireApiTokenService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequireApiTokenService {
            inner,
            store: Arc::clone(&self.store),
        }
    }
}

/// Tower service produced by [`RequireApiToken`].
#[derive(Clone)]
pub struct RequireApiTokenService<S> {
    inner: S,
    store: Arc<dyn ApiTokenStore>,
}

impl<S, ResBody> tower::Service<axum::extract::Request> for RequireApiTokenService<S>
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

    fn call(&mut self, mut req: axum::extract::Request) -> Self::Future {
        let store = Arc::clone(&self.store);
        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            // Parse "Authorization: Bearer <token>"
            let raw_token = req
                .headers()
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(str::to_owned);

            let Some(raw_token) = raw_token else {
                let (request_id, instance) = api_token_problem_context(&req);
                return Ok(api_token_unauthorized_response(request_id, instance));
            };

            match store.verify(&raw_token).await {
                Ok(Some(principal_id)) => {
                    req.extensions_mut().insert(ApiTokenPrincipal(principal_id));
                    inner.call(req).await
                }
                Ok(None) => {
                    let (request_id, instance) = api_token_problem_context(&req);
                    Ok(api_token_unauthorized_response(request_id, instance))
                }
                Err(err) => {
                    let (request_id, instance) = api_token_problem_context(&req);
                    Ok(api_token_error_response(&err, request_id, instance))
                }
            }
        })
    }
}

/// Build a `401 Unauthorized` response using the standard Problem Details body.
fn api_token_unauthorized_response<ResBody: From<String> + Default>(
    request_id: Option<String>,
    instance: Option<String>,
) -> Response<ResBody> {
    let body = crate::error::problem_details_json_string(
        StatusCode::UNAUTHORIZED,
        "authentication required",
        None,
        None,
        request_id,
        instance,
        true,
    );
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(http::header::CONTENT_TYPE, "application/problem+json")
        .body(ResBody::from(body))
        .unwrap_or_default()
}

/// Build a Problem Details response from the API token store error.
fn api_token_error_response<ResBody: From<String> + Default>(
    err: &crate::AutumnError,
    request_id: Option<String>,
    instance: Option<String>,
) -> Response<ResBody> {
    let status = err.status();
    let message = err.to_string();
    let body = crate::error::problem_details_json_string(
        status,
        message.clone(),
        None,
        None,
        request_id,
        instance,
        true,
    );
    let mut response = Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/problem+json")
        .body(ResBody::from(body))
        .unwrap_or_default();
    response
        .extensions_mut()
        .insert(crate::middleware::AutumnErrorInfo {
            status,
            message,
            details: None,
            problem_type: None,
        });
    response
}

fn api_token_problem_context(req: &axum::extract::Request) -> (Option<String>, Option<String>) {
    (
        req.extensions()
            .get::<crate::middleware::RequestId>()
            .map(std::string::ToString::to_string),
        Some(req.uri().path().to_owned()),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Diesel-backed API Token Store
// ─────────────────────────────────────────────────────────────────────────────

/// Embedded Diesel migrations for the `api_tokens` table.
///
/// Include this in your application's `.migrations()` call so that the
/// `api_tokens` table is created and kept up-to-date alongside your own
/// migrations:
///
/// ```rust,ignore
/// use autumn_web::auth::API_TOKEN_MIGRATIONS;
///
/// #[autumn_web::main]
/// async fn main() {
///     autumn_web::app()
///         .migrations(API_TOKEN_MIGRATIONS)
///         .run()
///         .await;
/// }
/// ```
#[cfg(feature = "db")]
pub const API_TOKEN_MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("migrations");

#[cfg(feature = "db")]
mod db_store {
    use std::future::Future;
    use std::pin::Pin;

    use diesel::OptionalExtension as _;
    use diesel::prelude::*;
    use diesel_async::AsyncPgConnection;
    use diesel_async::RunQueryDsl;
    use diesel_async::pooled_connection::deadpool::Pool;

    use super::{ApiTokenStore, generate_raw_token, hash_api_token};
    use crate::error::AutumnError;

    diesel::table! {
        api_tokens (id) {
            id -> Int8,
            token_hash -> Text,
            principal_id -> Text,
            created_at -> Timestamp,
            revoked_at -> Nullable<Timestamp>,
        }
    }

    #[derive(Insertable)]
    #[diesel(table_name = api_tokens)]
    struct NewApiToken<'a> {
        token_hash: &'a str,
        principal_id: &'a str,
    }

    /// Postgres-backed [`ApiTokenStore`].
    ///
    /// Tokens are hashed at rest (SHA-256) and never stored in plaintext.
    /// Suitable for production deployments where token state must survive
    /// process restarts and be shared across instances.
    ///
    /// # Setup
    ///
    /// Pass [`super::API_TOKEN_MIGRATIONS`] to your app builder so the
    /// `api_tokens` table is created automatically:
    ///
    /// ```rust,ignore
    /// use autumn_web::auth::{API_TOKEN_MIGRATIONS, DbApiTokenStore};
    /// use autumn_web::db::Pool;
    ///
    /// let store = DbApiTokenStore::new(pool.clone());
    /// autumn_web::app()
    ///     .migrations(API_TOKEN_MIGRATIONS)
    ///     .run()
    ///     .await;
    /// ```
    #[derive(Clone)]
    pub struct DbApiTokenStore {
        pool: Pool<AsyncPgConnection>,
    }

    impl DbApiTokenStore {
        /// Create a [`DbApiTokenStore`] backed by `pool`.
        #[must_use]
        pub const fn new(pool: Pool<AsyncPgConnection>) -> Self {
            Self { pool }
        }
    }

    impl ApiTokenStore for DbApiTokenStore {
        fn issue<'a>(
            &'a self,
            principal_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<String>> + Send + 'a>> {
            Box::pin(async move {
                let raw = generate_raw_token();
                let hash = hash_api_token(&raw);
                let mut conn = self
                    .pool
                    .get()
                    .await
                    .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;
                diesel::insert_into(api_tokens::table)
                    .values(NewApiToken {
                        token_hash: &hash,
                        principal_id,
                    })
                    .execute(&mut conn)
                    .await
                    .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;
                Ok(raw)
            })
        }

        fn verify<'a>(
            &'a self,
            raw_token: &'a str,
        ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<Option<String>>> + Send + 'a>>
        {
            Box::pin(async move {
                let hash = hash_api_token(raw_token);
                let mut conn = self
                    .pool
                    .get()
                    .await
                    .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;
                let principal: Option<String> = api_tokens::table
                    .filter(api_tokens::token_hash.eq(&hash))
                    .filter(api_tokens::revoked_at.is_null())
                    .select(api_tokens::principal_id)
                    .first(&mut conn)
                    .await
                    .optional()
                    .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;
                Ok(principal)
            })
        }

        fn revoke<'a>(
            &'a self,
            raw_token: &'a str,
        ) -> Pin<Box<dyn Future<Output = crate::AutumnResult<()>> + Send + 'a>> {
            Box::pin(async move {
                let hash = hash_api_token(raw_token);
                let mut conn = self
                    .pool
                    .get()
                    .await
                    .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;
                diesel::update(api_tokens::table)
                    .filter(api_tokens::token_hash.eq(&hash))
                    .set(api_tokens::revoked_at.eq(diesel::dsl::now.nullable()))
                    .execute(&mut conn)
                    .await
                    .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;
                Ok(())
            })
        }
    }
}

#[cfg(feature = "db")]
pub use db_store::DbApiTokenStore;

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

    #[tokio::test]
    async fn verify_password_rejects_invalid_hash_format_safely() {
        // Test short hash
        let result = verify_password("test", "short").await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        // Test hash with correct length but not starting with $
        let bad_prefix = "a".repeat(60);
        let result = verify_password("test", &bad_prefix).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        // Test hash with incorrect length but starting with $
        let bad_length = "$2b$12$short";
        let result = verify_password("test", bad_length).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn auth_config_defaults() {
        let config = AuthConfig::default();
        assert_eq!(config.bcrypt_cost, 12);
        assert_eq!(config.session_key, "user_id");
        #[cfg(feature = "oauth2")]
        assert!(config.oauth2.providers.is_empty());
    }

    #[cfg(feature = "oauth2")]
    #[test]
    fn oauth2_config_deserializes_provider_tables() {
        let cfg: crate::config::AutumnConfig = toml::from_str(
            r#"
            [auth.oauth2.github]
            client_id = "cid"
            client_secret = "secret"
            authorize_url = "https://github.com/login/oauth/authorize"
            token_url = "https://github.com/login/oauth/access_token"
            redirect_uri = "http://localhost:3000/auth/github/callback"
            "#,
        )
        .unwrap();
        let provider = cfg.auth.oauth2.providers.get("github").unwrap();
        assert_eq!(provider.client_id, "cid");
        assert_eq!(provider.scope, "");
        assert!(provider.issuer.is_none());
        assert!(provider.jwks_url.is_none());
    }

    #[cfg(feature = "oauth2")]
    #[tokio::test]
    async fn oauth2_authorize_url_sets_state_and_nonce() {
        let session = crate::session::Session::new_for_test("s1".into(), HashMap::new());
        let provider = OAuth2ProviderConfig {
            client_id: "cid".into(),
            client_secret: "secret".into(),
            authorize_url: "https://idp.example/authorize".into(),
            token_url: "https://idp.example/token".into(),
            userinfo_url: None,
            redirect_uri: "http://localhost:3000/callback".into(),
            scope: "openid profile".into(),
            issuer: None,
            jwks_url: None,
        };
        let url = oauth2_authorize_url(&session, "github", &provider)
            .await
            .unwrap();
        assert!(url.contains("response_type=code"));
        assert!(session.get("oauth2:github:state").await.is_some());
        assert!(session.get("oauth2:github:nonce").await.is_some());
    }

    #[cfg(feature = "oauth2")]
    #[tokio::test]
    async fn oauth2_authorize_url_omits_scope_when_empty() {
        let session = crate::session::Session::new_for_test("s1".into(), HashMap::new());
        let provider = OAuth2ProviderConfig {
            client_id: "cid".into(),
            client_secret: "secret".into(),
            authorize_url: "https://idp.example/authorize".into(),
            token_url: "https://idp.example/token".into(),
            userinfo_url: None,
            redirect_uri: "http://localhost:3000/callback".into(),
            scope: String::new(),
            issuer: None,
            jwks_url: None,
        };
        let url = oauth2_authorize_url(&session, "github", &provider)
            .await
            .unwrap();
        assert!(!url.contains("scope="));
    }

    #[cfg(feature = "oauth2")]
    #[tokio::test]
    async fn validate_id_token_requires_oidc_metadata() {
        let provider = OAuth2ProviderConfig {
            client_id: "cid".into(),
            client_secret: "secret".into(),
            authorize_url: "https://idp.example/authorize".into(),
            token_url: "https://idp.example/token".into(),
            userinfo_url: None,
            redirect_uri: "http://localhost:3000/callback".into(),
            scope: "openid profile".into(),
            issuer: None,
            jwks_url: None,
        };
        let err = validate_and_decode_id_token("bad.token.value", &provider)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "provider.issuer required for oidc");
    }

    #[cfg(feature = "oauth2")]
    #[test]
    fn parse_oauth2_token_response_supports_form_encoded_payload() {
        let token = parse_oauth2_token_response(
            Some("application/x-www-form-urlencoded"),
            "access_token=abc123&token_type=bearer&id_token=xyz789&extra_field=ignored",
        )
        .unwrap();
        assert_eq!(token.access_token, "abc123");
        assert_eq!(token.token_type.as_deref(), Some("bearer"));
        assert_eq!(token.id_token.as_deref(), Some("xyz789"));
    }

    #[cfg(feature = "oauth2")]
    #[test]
    fn parse_oauth2_token_response_fails_without_access_token() {
        let err = parse_oauth2_token_response(
            Some("application/x-www-form-urlencoded"),
            "token_type=bearer&id_token=xyz789",
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "token response missing access_token");
    }

    #[cfg(feature = "oauth2")]
    #[test]
    fn extract_subject_allows_userinfo_id_fallback() {
        let claims = serde_json::json!({ "id": 42 });
        let subject = extract_subject(&claims, IdentitySource::UserInfo).unwrap();
        assert_eq!(subject, "42");
    }

    #[cfg(feature = "oauth2")]
    #[tokio::test]
    async fn validate_callback_state_preserves_state_on_mismatch() {
        // An attacker hitting the callback with a wrong state must NOT
        // consume the real state stored in the session; the legitimate
        // provider redirect must still succeed.
        let session = crate::session::Session::new_for_test("s1".into(), HashMap::new());
        session
            .insert("oauth2:github:state".to_owned(), "real-state".to_owned())
            .await;
        let bad_callback = OAuth2Callback {
            code: "c".into(),
            state: "wrong-state".into(),
        };
        let err = validate_callback_state(&session, "github", &bad_callback)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("state mismatch"));
        // Real state must still be present after the failed attempt.
        assert_eq!(
            session.get("oauth2:github:state").await.as_deref(),
            Some("real-state")
        );
    }

    #[cfg(feature = "oauth2")]
    #[tokio::test]
    async fn validate_oidc_nonce_rejects_missing_nonce_for_id_token() {
        // ID-token logins must fail when there is no stored nonce (e.g.,
        // session was partially cleared or forged).
        let session = crate::session::Session::new_for_test("s1".into(), HashMap::new());
        // No nonce key inserted — simulates a cleared / missing session.
        let claims = serde_json::json!({ "nonce": "any" });
        let err = validate_oidc_nonce(&session, "github", &claims, IdentitySource::IdToken)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nonce missing from session"));
    }

    #[cfg(feature = "oauth2")]
    #[test]
    fn extract_subject_requires_sub_for_id_token() {
        let claims = serde_json::json!({ "id": "abc" });
        let err = extract_subject(&claims, IdentitySource::IdToken).unwrap_err();
        assert_eq!(err.to_string(), "missing sub claim");
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
        assert_eq!(err.to_string(), "authentication required");
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
        assert_eq!(err.to_string(), "insufficient permissions");
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
    async fn secured_macro_honors_configured_auth_session_key() {
        use axum::Router;
        use axum::body::Body;
        use axum::routing::get;
        use http::header::COOKIE;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer, SessionStore};
        use crate::state::AppState;

        #[autumn_macros::secured]
        async fn account_handler() -> crate::AutumnResult<&'static str> {
            Ok("account")
        }

        let store = MemoryStore::new();
        store
            .save(
                "sess1",
                std::collections::HashMap::from([
                    ("uid".into(), "42".into()),
                    ("account_id".into(), "42".into()),
                ]),
            )
            .await
            .unwrap();

        let state = AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "uid".to_owned(),
            shared_cache: None,
        };

        let app = Router::new()
            .route("/account", get(account_handler))
            .layer(SessionLayer::new(store, SessionConfig::default()))
            .with_state(state);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/account")
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
        assert_eq!(std::str::from_utf8(&body).unwrap(), "account");
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: false,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
            shared_cache: None,
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
            poll_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }

        impl Service<axum::extract::Request> for MockService {
            type Response = axum::response::Response;
            type Error = std::convert::Infallible;
            type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                self.poll_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        let poll_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mock_service = MockService {
            ready: false,
            poll_count: poll_count.clone(),
        };
        let mut service = layer.layer(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // When inner is not ready, RequireAuthService should not be ready
        let poll = service.poll_ready(&mut cx);
        assert!(poll.is_pending());
        assert_eq!(poll_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        // When inner is ready, RequireAuthService should be ready
        let mock_service_ready = MockService {
            ready: true,
            poll_count: poll_count.clone(),
        };
        let mut service_ready = layer.layer(mock_service_ready);
        let poll_ready = service_ready.poll_ready(&mut cx);
        assert!(poll_ready.is_ready());
        assert_eq!(poll_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn auth_rejection_into_response() {
        let rejection = AuthRejection;
        let response = rejection.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], 401);
        assert_eq!(json["detail"], "authentication required");
        assert_eq!(json["code"], "autumn.unauthorized");
    }

    #[test]
    fn test_auth_config_defaults() {
        let config = AuthConfig::default();
        assert_eq!(config.bcrypt_cost, DEFAULT_BCRYPT_COST);
        assert_eq!(config.session_key, "user_id");
    }

    #[tokio::test]
    async fn test_hash_password() {
        let test_input = uuid::Uuid::new_v4().to_string();

        // Test hashing
        let hash = super::hash_password(&test_input)
            .await
            .expect("Failed to hash password");
        assert!(hash.starts_with("$2b$"));

        // Test verification with correct password
        let is_valid = super::verify_password(&test_input, &hash)
            .await
            .expect("Failed to verify password");
        assert!(is_valid, "Password should be verified successfully");

        // Test verification with incorrect password
        let is_invalid = super::verify_password(&uuid::Uuid::new_v4().to_string(), &hash)
            .await
            .expect("Failed to verify wrong password");
        assert!(!is_invalid, "Wrong password should not be verified");
    }

    #[tokio::test]
    async fn test_hash_password_empty() {
        let test_input = String::new();
        let hash = super::hash_password(&test_input)
            .await
            .expect("Failed to hash empty password");
        assert!(hash.starts_with("$2b$"));

        let is_valid = super::verify_password(&test_input, &hash)
            .await
            .expect("Failed to verify empty password");
        assert!(is_valid, "Empty password should be verified successfully");
    }

    #[tokio::test]
    async fn test_hash_password_long() {
        // bcrypt truncates after 72 bytes. We just want to ensure it doesn't crash.
        let test_input = "a".repeat(100);
        let hash = super::hash_password(&test_input)
            .await
            .expect("Failed to hash long password");
        assert!(hash.starts_with("$2b$"));

        let is_valid = super::verify_password(&test_input, &hash)
            .await
            .expect("Failed to verify long password");
        assert!(is_valid, "Long password should be verified successfully");
    }

    #[tokio::test]
    async fn test_hash_password_unicode() {
        // Test with non-ascii characters
        let test_input = format!("{}🚀my_secrët_passwörd🔑", uuid::Uuid::new_v4());
        let hash = super::hash_password(&test_input)
            .await
            .expect("Failed to hash unicode password");
        assert!(hash.starts_with("$2b$"));

        let is_valid = super::verify_password(&test_input, &hash)
            .await
            .expect("Failed to verify unicode password");
        assert!(is_valid, "Unicode password should be verified successfully");
    }

    #[tokio::test]
    async fn test_verify_password_invalid_hash() {
        // Ensure that providing invalid hashes doesn't crash or cause issues, but returns an error/false
        let test_input = uuid::Uuid::new_v4().to_string();

        // Invalid prefix
        let result = super::verify_password(&test_input, "invalid_hash_string").await;
        assert!(result.is_err() || !result.unwrap());

        // Truncated hash
        let result2 = super::verify_password(&test_input, "$2b$04$").await;
        assert!(result2.is_err() || !result2.unwrap());
    }
}

// ── API token tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod api_token_tests {
    use std::sync::Arc;

    use http::StatusCode;

    use super::{
        ApiToken, ApiTokenStore, InMemoryApiTokenStore, RequireApiToken, hash_api_token,
        issue_api_token, revoke_api_token,
    };

    struct FailingApiTokenStore;

    impl ApiTokenStore for FailingApiTokenStore {
        fn issue<'a>(
            &'a self,
            _principal_id: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::AutumnResult<String>> + Send + 'a>,
        > {
            Box::pin(async {
                Err(crate::AutumnError::service_unavailable_msg(
                    "api token store unavailable",
                ))
            })
        }

        fn verify<'a>(
            &'a self,
            _raw_token: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::AutumnResult<Option<String>>> + Send + 'a>,
        > {
            Box::pin(async {
                Err(crate::AutumnError::service_unavailable_msg(
                    "api token store unavailable",
                ))
            })
        }

        fn revoke<'a>(
            &'a self,
            _raw_token: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>>
        {
            Box::pin(async {
                Err(crate::AutumnError::service_unavailable_msg(
                    "api token store unavailable",
                ))
            })
        }
    }

    // ── hash_api_token ───────────────────────────────────────────────────────

    #[test]
    fn hash_api_token_is_deterministic() {
        let h1 = hash_api_token("abc123");
        let h2 = hash_api_token("abc123");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_api_token_produces_64_char_hex() {
        let hash = hash_api_token("any_raw_token");
        assert_eq!(hash.len(), 64, "SHA-256 hex must be 64 chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be lowercase hex digits"
        );
    }

    #[test]
    fn hash_api_token_differs_from_input() {
        let raw = "my_raw_token";
        assert_ne!(hash_api_token(raw), raw);
    }

    #[test]
    fn hash_api_token_different_inputs_produce_different_hashes() {
        assert_ne!(hash_api_token("token_a"), hash_api_token("token_b"));
    }

    // ── InMemoryApiTokenStore ────────────────────────────────────────────────

    #[tokio::test]
    async fn in_memory_store_issue_returns_unique_tokens() {
        let store = InMemoryApiTokenStore::default();
        let t1 = store.issue("user:1").await.unwrap();
        let t2 = store.issue("user:1").await.unwrap();
        assert_ne!(t1, t2, "each issued token must be unique");
        assert!(t1.len() >= 32, "token must have sufficient entropy");
    }

    #[tokio::test]
    async fn in_memory_store_verify_returns_principal_for_valid_token() {
        let store = InMemoryApiTokenStore::default();
        let raw = store.issue("user:42").await.unwrap();
        let principal = store.verify(&raw).await.unwrap();
        assert_eq!(principal, Some("user:42".to_owned()));
    }

    #[tokio::test]
    async fn in_memory_store_verify_returns_none_for_unknown_token() {
        let store = InMemoryApiTokenStore::default();
        let result = store.verify("not_a_real_token").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn in_memory_store_revoke_invalidates_token() {
        let store = InMemoryApiTokenStore::default();
        let raw = store.issue("user:7").await.unwrap();
        assert_eq!(
            store.verify(&raw).await.unwrap(),
            Some("user:7".to_owned()),
            "token must be valid before revoking"
        );
        store.revoke(&raw).await.unwrap();
        assert_eq!(store.verify(&raw).await.unwrap(), None);
    }

    #[tokio::test]
    async fn in_memory_store_raw_token_not_stored_verbatim() {
        let store = InMemoryApiTokenStore::default();
        let raw = store.issue("user:1").await.unwrap();
        // Appending a character changes the hash → lookup must return None.
        let tampered = format!("{raw}x");
        assert_eq!(store.verify(&tampered).await.unwrap(), None);
    }

    #[tokio::test]
    async fn issue_api_token_helper_issues_verifiable_token() {
        let store = InMemoryApiTokenStore::default();
        let raw = issue_api_token(&store, "user:5").await.unwrap();
        assert_eq!(store.verify(&raw).await.unwrap(), Some("user:5".to_owned()));
    }

    #[tokio::test]
    async fn revoke_api_token_helper_revokes_token() {
        let store = InMemoryApiTokenStore::default();
        let raw = store.issue("user:6").await.unwrap();
        revoke_api_token(&store, &raw).await.unwrap();
        assert_eq!(store.verify(&raw).await.unwrap(), None);
    }

    // ── RequireApiToken middleware ───────────────────────────────────────────

    #[tokio::test]
    async fn require_api_token_rejects_missing_authorization_header() {
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(InMemoryApiTokenStore::default());
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(store));

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
    async fn require_api_token_rejects_non_bearer_scheme() {
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(InMemoryApiTokenStore::default());
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(store));

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(http::header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_api_token_rejects_unknown_bearer_token() {
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(InMemoryApiTokenStore::default());
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(store));

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(http::header::AUTHORIZATION, "Bearer unknown_token_xyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_api_token_propagates_store_verify_errors() {
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(FailingApiTokenStore);
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(store));

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(http::header::AUTHORIZATION, "Bearer valid_client_token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .map(|value| value.to_str().unwrap_or_default()),
            Some("application/problem+json")
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], 503);
        assert_eq!(json["code"], "autumn.service_unavailable");
        assert_eq!(json["detail"], "api token store unavailable");
    }

    #[tokio::test]
    async fn require_api_token_allows_valid_bearer_token() {
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(InMemoryApiTokenStore::default());
        let raw = store.issue("user:1").await.unwrap();
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(Arc::clone(&store)));

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(http::header::AUTHORIZATION, format!("Bearer {raw}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn require_api_token_rejects_revoked_token() {
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(InMemoryApiTokenStore::default());
        let raw = store.issue("user:1").await.unwrap();
        store.revoke(&raw).await.unwrap();
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(Arc::clone(&store)));

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(http::header::AUTHORIZATION, format!("Bearer {raw}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_api_token_401_response_has_problem_details() {
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(InMemoryApiTokenStore::default());
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(store));

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
        assert_eq!(
            response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or_default()),
            Some("application/problem+json")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], 401);
        assert_eq!(json["code"], "autumn.unauthorized");
        assert!(json["detail"].as_str().is_some());
    }

    #[tokio::test]
    async fn require_api_token_401_problem_details_include_request_context() {
        use crate::middleware::RequestIdLayer;
        use axum::body::Body;
        use tower::ServiceExt;

        let store = Arc::new(InMemoryApiTokenStore::default());
        let app = axum::Router::new()
            .route("/api/private", axum::routing::get(|| async { "ok" }))
            .layer(RequireApiToken::new(store))
            .layer(RequestIdLayer);

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/api/private")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .expect("request id header should be present")
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["request_id"], request_id);
        assert_eq!(json["instance"], "/api/private");
    }

    // ── ApiToken extractor ───────────────────────────────────────────────────

    #[tokio::test]
    async fn api_token_extractor_yields_principal_id_to_handler() {
        use axum::body::Body;
        use tower::ServiceExt;

        async fn handler(ApiToken(principal): ApiToken) -> String {
            principal
        }

        let store = Arc::new(InMemoryApiTokenStore::default());
        let raw = store.issue("user:99").await.unwrap();
        let app = axum::Router::new()
            .route("/", axum::routing::get(handler))
            .layer(RequireApiToken::new(Arc::clone(&store)));

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header(http::header::AUTHORIZATION, format!("Bearer {raw}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "user:99");
    }

    #[tokio::test]
    async fn api_token_extractor_rejects_when_no_principal_in_extensions() {
        use axum::body::Body;
        use tower::ServiceExt;

        async fn handler(ApiToken(principal): ApiToken) -> String {
            principal
        }

        let app = axum::Router::new().route("/", axum::routing::get(handler));

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

    // ── Composition with session auth ────────────────────────────────────────

    #[tokio::test]
    async fn api_token_and_session_auth_compose_without_conflict() {
        use axum::body::Body;
        use tower::ServiceExt;

        use crate::session::{MemoryStore, SessionConfig, SessionLayer, SessionStore};

        async fn api_handler(ApiToken(principal): ApiToken) -> String {
            principal
        }

        let store = Arc::new(InMemoryApiTokenStore::default());
        let raw = store.issue("api_user").await.unwrap();

        let session_store = MemoryStore::new();
        session_store
            .save(
                "sess1",
                std::collections::HashMap::from([("user_id".into(), "session_user".into())]),
            )
            .await
            .unwrap();

        let app = axum::Router::new()
            .route(
                "/api",
                axum::routing::get(api_handler).layer(RequireApiToken::new(Arc::clone(&store))),
            )
            .layer(SessionLayer::new(session_store, SessionConfig::default()));

        let response = app
            .oneshot(
                http::Request::builder()
                    .uri("/api")
                    .header(http::header::AUTHORIZATION, format!("Bearer {raw}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "api_user");
    }

    // ── poll_ready propagation ───────────────────────────────────────────────

    #[tokio::test]
    async fn require_api_token_poll_ready_propagates_to_inner() {
        use std::task::{Context, Poll};
        use tower::{Layer, Service};

        #[derive(Clone)]
        struct MockService {
            ready: bool,
        }

        impl tower::Service<axum::extract::Request> for MockService {
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

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let store = Arc::new(InMemoryApiTokenStore::default());
        let layer = RequireApiToken::new(store);
        let mut svc = layer.layer(MockService { ready: false });
        assert!(svc.poll_ready(&mut cx).is_pending());

        let store2 = Arc::new(InMemoryApiTokenStore::default());
        let layer2 = RequireApiToken::new(store2);
        let mut svc2 = layer2.layer(MockService { ready: true });
        assert!(svc2.poll_ready(&mut cx).is_ready());
    }
}
