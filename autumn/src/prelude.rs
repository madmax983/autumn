//! Convenience re-exports for Autumn applications.
//!
//! Import everything commonly needed with a single glob:
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! ```
//!
//! This brings the following into scope:
//!
//! | Category | Items |
//! |----------|-------|
//! | Route macros | [`get`], [`post`], [`put`], [`delete`], [`routes`], [`main`] |
//! | HTML rendering | [`Markup`], [`PreEscaped`], [`html!`](maud::html) |
//! | Extractors | [`Db`], [`Json`], [`Form`], [`Path`], [`Query`] |
//! | Error handling | [`AutumnError`], [`AutumnResult`] |
//! | State | [`AppState`] |
//!
//! For less common types (configuration, middleware, upstream crate access),
//! use targeted imports from [`autumn_web::config`](crate::config),
//! [`autumn_web::middleware`](crate::middleware), or
//! [`autumn_web::reexports`](crate::reexports).

// ── Route macros ─────────────────────────────────────────────────
/// HTTP redirect response.
pub use crate::Redirect;
/// Typed path helper extension trait (`.with_query()`).
pub use crate::paths::PathExt;
#[cfg(feature = "mail")]
pub use autumn_macros::mailer;
/// WebSocket route macro.
#[cfg(feature = "ws")]
pub use autumn_macros::ws;
/// HTTP method route macros, main macro, and route collection.
pub use autumn_macros::{
    api_doc, authorize, cached, delete, get, job, jobs, main, oauth2_callback, patch, paths, post,
    put, routes, scheduled, secured, service, static_get, static_routes, tasks,
};

// ── Rendering ────────────────────────────────────────────────────
/// Resolve a logical static asset path to a fingerprinted URL in release builds.
pub use crate::assets::asset_url;
/// Maud HTML templating types.
#[cfg(feature = "maud")]
pub use maud::{Markup, PreEscaped, html};

// ── Extractors ───────────────────────────────────────────────────
/// Database connection extractor.
#[cfg(feature = "db")]
pub use crate::db::Db;
/// Form data extractor.
pub use crate::extract::Form;
/// JSON request/response type.
pub use crate::extract::Json;
/// Multipart extractor with upload policy helpers.
#[cfg(feature = "multipart")]
pub use crate::extract::Multipart;
/// Path extractor.
pub use crate::extract::Path;
/// Query extractor.
pub use crate::extract::Query;
/// Flash message extractor.
#[cfg(feature = "flash")]
pub use crate::flash::{Flash, FlashLevel, FlashMessage};
/// Extension trait for adding htmx response headers.
#[cfg(feature = "htmx")]
pub use crate::htmx::HxResponseExt;
/// htmx request extractor.
#[cfg(feature = "htmx")]
pub use crate::htmx::{HTMX_CSRF_JS_PATH, HTMX_JS_PATH, HxRequest};
/// Transactional email types and extractor.
#[cfg(feature = "mail")]
pub use crate::mail::{
    Mail, MailConfig, MailError, MailTransport, Mailer, SmtpConfig, TlsMode, Transport,
};
/// Server-Sent Events (SSE) support.
pub use crate::sse::{Event, Sse};
/// State extractor.
pub use axum::extract::State;
/// Trait for types that can be converted into an HTTP response.
pub use axum::response::IntoResponse;
/// HTTP status codes.
pub use http::StatusCode;

// ── Error handling ───────────────────────────────────────────────
/// Structured audit event types.
pub use crate::audit::{AuditEvent, AuditStatus};
/// Framework error and result types.
pub use crate::error::{AutumnError, AutumnResult};

// ── Pagination ──────────────────────────────────────────────────
/// Pagination primitives — offset and cursor extractors and wrappers.
pub use crate::pagination::{CursorPage, CursorRequest, Page, PageRequest};

// ── Validation ──────────────────────────────────────────────────
/// Auto-validating extractor and proof-of-validation newtype.
pub use crate::validation::{Valid, ValidateExt, Validated};
/// Validation trait — derive with `#[derive(Validate)]` on form/model types.
pub use validator::Validate;

// ── Form ─────────────────────────────────────────────────────────
/// Changeset-style form helpers: [`Changeset`], [`ChangesetForm`], [`IntoChangeset`].
///
/// See [`crate::form`] for the full surface including Maud rendering helpers.
pub use crate::form::{Changeset, ChangesetForm, IntoChangeset};

// ── Hooks ───────────────────────────────────────────────────────
/// Mutation hook types for repository lifecycle callbacks.
#[cfg(feature = "db")]
pub use crate::hooks::{
    DraftField, FieldDiff, MutationContext, MutationHooks, MutationOp, Patch, UpdateDraft,
};

// ── Session & Auth ──────────────────────────────────────────────
/// Auth extractor for retrieving the authenticated user (session-based).
pub use crate::auth::Auth;
/// Extractor for the verified principal ID on bearer-token-protected routes.
pub use crate::auth::ApiToken;
/// Tower layer that validates `Authorization: Bearer <token>` on API routes.
pub use crate::auth::RequireApiToken;
/// Session extractor for accessing per-user session data.
pub use crate::session::Session;

// ── Authorization ────────────────────────────────────────────────
/// Record-level authorization primitives. See
/// [`crate::authorization`] for the full surface.
pub use crate::authorization::{Policy, PolicyContext, Scope, ScopeQuery, Scoped};

// ── Security ───────────────────────────────────────────────────
/// CSRF token extractor for embedding in forms.
pub use crate::security::CsrfToken;

// ── Application state ────────────────────────────────────────────
/// Shared application state (for custom extractors).
pub use crate::state::AppState;

// ── Internationalization ───────────────────────────────────────
/// Request-scoped locale extractor (resolves from query, cookie,
/// `Accept-Language`, and default in that order).
#[cfg(feature = "i18n")]
pub use crate::i18n::Locale;
/// Translation lookup macro with compile-time key validation — see
/// [`crate::i18n`] for usage.
#[cfg(feature = "i18n")]
pub use crate::i18n::t;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_types_are_accessible() {
        #[cfg(feature = "db")]
        let _state = AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            pool: None,
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
        };
        #[cfg(not(feature = "db"))]
        let _state = AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
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
        };
        let _err: AutumnResult<()> = Ok(());
    }

    #[test]
    fn json_type_works_through_prelude() {
        let json: Json<&str> = Json("ok");
        assert_eq!(json.0, "ok");
    }

    #[test]
    fn error_types_work_through_prelude() {
        let err = AutumnError::bad_request_msg("test");
        let result: AutumnResult<()> = Err(err);
        assert!(result.is_err());
    }

    #[cfg(feature = "maud")]
    #[test]
    fn maud_types_work_through_prelude() {
        let markup: Markup = html! { "hello" };
        assert!(markup.into_string().contains("hello"));
    }
}
