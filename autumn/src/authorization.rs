//! Policy-based record-level authorization.
//!
//! Spring Security's role checks (`#[secured("admin")]`) answer
//! "are you allowed to call this *route*?" This module answers
//! "are you allowed to act on *this specific record*?" — the
//! question every multi-user CRUD app has to answer at every write
//! endpoint.
//!
//! The shape mirrors Pundit (Rails) and Bodyguard (Phoenix): one
//! [`Policy`] impl per resource, a [`Scope`] companion for list
//! queries, default-deny semantics, and an `#[authorize]` attribute
//! macro that wires the check declaratively.
//!
//! # Quick start
//!
//! ```rust,ignore
//! use autumn_web::authorization::{Policy, PolicyContext};
//! use autumn_web::AutumnResult;
//!
//! #[derive(Default)]
//! pub struct PostPolicy;
//!
//! impl Policy<Post> for PostPolicy {
//!     fn can_show<'a>(&'a self, _ctx: &'a PolicyContext, _post: &'a Post)
//!         -> autumn_web::authorization::BoxFuture<'a, bool>
//!     {
//!         Box::pin(async { true }) // posts are public
//!     }
//!
//!     fn can_update<'a>(&'a self, ctx: &'a PolicyContext, post: &'a Post)
//!         -> autumn_web::authorization::BoxFuture<'a, bool>
//!     {
//!         Box::pin(async move {
//!             ctx.has_role("admin")
//!                 || ctx.user_id_i64() == Some(post.author_id)
//!         })
//!     }
//!
//!     fn can_delete<'a>(&'a self, ctx: &'a PolicyContext, post: &'a Post)
//!         -> autumn_web::authorization::BoxFuture<'a, bool>
//!     {
//!         Box::pin(async move {
//!             ctx.has_role("admin")
//!                 || ctx.user_id_i64() == Some(post.author_id)
//!         })
//!     }
//! }
//! ```
//!
//! Register the policy on the app builder and reference it from
//! `#[repository(api = "/posts", policy = PostPolicy)]` to enforce
//! the same checks on auto-generated REST endpoints.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use http::StatusCode;

use crate::session::Session;

/// Boxed future returned by [`Policy`] and [`Scope`] methods so the
/// traits remain object-safe (`dyn Policy<R>` works regardless of
/// rust edition).
pub type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

// ── PolicyContext ────────────────────────────────────────────────

/// Per-request context handed to every policy and scope check.
///
/// Carries the resolved [`Session`], the authenticated user id (when
/// present), the active role set, and a clone of the database pool
/// so policies can consult related rows. The struct is `Clone +
/// Send + Sync` so it can flow freely across `.await` points.
#[derive(Clone)]
pub struct PolicyContext {
    /// The full per-request [`Session`]. Read raw values via
    /// [`Session::get`] when a policy needs data beyond the
    /// canonical user-id and role keys.
    pub session: Session,

    /// The authenticated user id, if any. Mirrors the configured
    /// session auth key (default: `"user_id"`).
    pub user_id: Option<String>,

    /// Active role set for the current user. Empty when the user
    /// has no role or is anonymous.
    pub roles: Vec<String>,

    /// Database connection pool, cloned from `AppState`. Policies
    /// that need to consult related rows (e.g. group membership)
    /// can borrow a connection here.
    #[cfg(feature = "db")]
    pub pool: Option<
        diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    >,
}

impl PolicyContext {
    /// Build a [`PolicyContext`] from request parts.
    ///
    /// Reads the session keys named in the supplied
    /// [`AuthConfig`](crate::auth::AuthConfig) (`session_key`,
    /// default `"user_id"`) plus the conventional `"role"` key.
    pub async fn from_session(session: &Session, auth_session_key: &str) -> Self {
        let user_id = session.get(auth_session_key).await;
        let role = session.get("role").await;
        let roles = role.into_iter().collect();
        Self {
            session: session.clone(),
            user_id,
            roles,
            #[cfg(feature = "db")]
            pool: None,
        }
    }

    /// Returns `true` when the request has a resolved authenticated user.
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.user_id.is_some()
    }

    /// Returns the user id parsed as an `i64`, when the session
    /// stored it as a numeric string. Convenient for the common
    /// case of `BIGSERIAL` primary keys.
    #[must_use]
    pub fn user_id_i64(&self) -> Option<i64> {
        self.user_id.as_deref().and_then(|s| s.parse().ok())
    }

    /// Returns `true` when the active role set contains `role`.
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Returns `true` when the active role set contains any of the
    /// supplied roles. Mirrors `#[secured("a", "b")]` semantics.
    #[must_use]
    pub fn has_any_role<I, S>(&self, candidates: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        candidates
            .into_iter()
            .any(|c| self.has_role(c.as_ref()))
    }

    /// Attach a database pool to the context. Used by the framework
    /// when constructing the context inside extractors; tests can
    /// also call this to inject a pool by hand.
    #[cfg(feature = "db")]
    #[must_use]
    pub fn with_pool(
        mut self,
        pool: diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    ) -> Self {
        self.pool = Some(pool);
        self
    }
}

// ── Policy trait ────────────────────────────────────────────────

/// Record-level authorization policy for a resource.
///
/// All four built-in actions (`can_show`, `can_create`,
/// `can_update`, `can_delete`) default to **denied**, which makes
/// opting into a policy safe-by-default: a freshly-introduced
/// policy with no overrides forbids every action until the
/// developer explicitly allows one.
///
/// # Object safety
///
/// Every method returns a [`BoxFuture`] so `dyn Policy<R>` is
/// usable behind an `Arc`. Tests can swap implementations via
/// `AppBuilder::policy::<R, P>(P::default())`.
///
/// # Custom verbs
///
/// Use [`Policy::can`] for verbs that are not one of the four
/// built-ins (e.g. `"publish"`, `"feature"`, `"archive"`). The
/// default impl dispatches the four built-ins and returns `false`
/// for unknown verbs.
pub trait Policy<R: Send + Sync + 'static>: Send + Sync + 'static {
    /// Decide whether the current user may *show* the resource.
    fn can_show<'a>(
        &'a self,
        _ctx: &'a PolicyContext,
        _resource: &'a R,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// Decide whether the current user may *create* a resource of
    /// this type. The `_resource` argument carries the proposed
    /// new value (or a sentinel value for shapes that have no pre-
    /// insert form).
    fn can_create<'a>(
        &'a self,
        _ctx: &'a PolicyContext,
        _resource: &'a R,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// Decide whether the current user may *update* the resource.
    fn can_update<'a>(
        &'a self,
        _ctx: &'a PolicyContext,
        _resource: &'a R,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// Decide whether the current user may *delete* the resource.
    fn can_delete<'a>(
        &'a self,
        _ctx: &'a PolicyContext,
        _resource: &'a R,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// Decide a custom verb. Defaults to dispatching the four
    /// built-ins by name.
    fn can<'a>(
        &'a self,
        action: &'a str,
        ctx: &'a PolicyContext,
        resource: &'a R,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            match action {
                "show" | "read" => self.can_show(ctx, resource).await,
                "create" => self.can_create(ctx, resource).await,
                "update" | "edit" => self.can_update(ctx, resource).await,
                "delete" | "destroy" => self.can_delete(ctx, resource).await,
                _ => false,
            }
        })
    }
}

// ── Scope trait ─────────────────────────────────────────────────

/// List-time companion to [`Policy`] for filtering record sets the
/// current user is allowed to read.
///
/// Default implementations return an **empty** list — fail closed.
/// `#[repository(policy = ...)]`-generated `GET /<api>` index
/// endpoints invoke the registered scope automatically; hand-
/// written list handlers can pull `Arc<dyn Scope<R>>` from
/// `AppState` and call `.list(&ctx).await`.
pub trait Scope<R: Send + Sync + 'static>: Send + Sync + 'static {
    /// Return the records the current user is allowed to read.
    ///
    /// The default impl returns `Ok(Vec::new())` so a missing
    /// scope opt-in fails closed.
    fn list<'a>(
        &'a self,
        _ctx: &'a PolicyContext,
    ) -> BoxFuture<'a, crate::AutumnResult<Vec<R>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

// ── PolicyRegistry ──────────────────────────────────────────────

/// Process-wide registry of resource → policy and resource →
/// scope bindings.
///
/// Stored on [`AppState`](crate::AppState) so handlers and
/// `#[repository]`-generated endpoints can resolve a policy by
/// resource type via [`AppState::policy::<R>`](crate::AppState::policy).
#[derive(Clone, Default)]
pub struct PolicyRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Default)]
struct RegistryInner {
    policies: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    scopes: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl PolicyRegistry {
    /// Register the [`Policy`] implementation for resource `R`.
    ///
    /// # Panics
    ///
    /// Panics if a policy is already registered for `R`. The issue
    /// spec is explicit: multiple policies per resource are not
    /// supported in this slice; double-registration must surface
    /// as a startup-time error rather than silent override.
    pub fn register_policy<R, P>(&self, policy: P)
    where
        R: Send + Sync + 'static,
        P: Policy<R>,
    {
        let mut inner = self
            .inner
            .write()
            .expect("policy registry lock poisoned");
        let key = TypeId::of::<R>();
        assert!(
            !inner.policies.contains_key(&key),
            "Policy for {} already registered. Multiple policies per resource are not supported.",
            std::any::type_name::<R>()
        );
        let boxed: Arc<dyn Policy<R>> = Arc::new(policy);
        inner.policies.insert(key, Arc::new(boxed));
    }

    /// Register the [`Scope`] implementation for resource `R`.
    ///
    /// # Panics
    ///
    /// Panics if a scope is already registered for `R`.
    pub fn register_scope<R, S>(&self, scope: S)
    where
        R: Send + Sync + 'static,
        S: Scope<R>,
    {
        let mut inner = self
            .inner
            .write()
            .expect("policy registry lock poisoned");
        let key = TypeId::of::<R>();
        assert!(
            !inner.scopes.contains_key(&key),
            "Scope for {} already registered. Multiple scopes per resource are not supported.",
            std::any::type_name::<R>()
        );
        let boxed: Arc<dyn Scope<R>> = Arc::new(scope);
        inner.scopes.insert(key, Arc::new(boxed));
    }

    /// Resolve the registered [`Policy`] for resource `R`.
    #[must_use]
    pub fn policy<R: Send + Sync + 'static>(&self) -> Option<Arc<dyn Policy<R>>> {
        let inner = self
            .inner
            .read()
            .expect("policy registry lock poisoned");
        inner
            .policies
            .get(&TypeId::of::<R>())
            .and_then(|a| a.downcast_ref::<Arc<dyn Policy<R>>>().cloned())
    }

    /// Resolve the registered [`Scope`] for resource `R`.
    #[must_use]
    pub fn scope<R: Send + Sync + 'static>(&self) -> Option<Arc<dyn Scope<R>>> {
        let inner = self
            .inner
            .read()
            .expect("policy registry lock poisoned");
        inner
            .scopes
            .get(&TypeId::of::<R>())
            .and_then(|a| a.downcast_ref::<Arc<dyn Scope<R>>>().cloned())
    }

    /// Returns `true` when a policy is registered for resource `R`.
    #[must_use]
    pub fn has_policy<R: Send + Sync + 'static>(&self) -> bool {
        self.inner
            .read()
            .expect("policy registry lock poisoned")
            .policies
            .contains_key(&TypeId::of::<R>())
    }
}

impl std::fmt::Debug for PolicyRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self
            .inner
            .read()
            .expect("policy registry lock poisoned");
        f.debug_struct("PolicyRegistry")
            .field("policies", &inner.policies.len())
            .field("scopes", &inner.scopes.len())
            .finish()
    }
}

// ── Forbidden response shape ────────────────────────────────────

/// HTTP status the framework returns when a [`Policy`] denies an
/// action.
///
/// Defaults to `404 Not Found` so unauthorized clients cannot
/// distinguish "the record exists but you cannot touch it" from
/// "the record does not exist." This mirrors Rails / Phoenix
/// defaults; flip to `403` via
/// `[security] forbidden_response = "403"` in `autumn.toml` when
/// the leak is acceptable (e.g. internal admin tooling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForbiddenResponse {
    /// Return `403 Forbidden`.
    Forbidden403,
    /// Return `404 Not Found` (default, hides existence).
    NotFound404,
}

impl Default for ForbiddenResponse {
    fn default() -> Self {
        Self::NotFound404
    }
}

impl ForbiddenResponse {
    /// HTTP status code for the deny response.
    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::Forbidden403 => StatusCode::FORBIDDEN,
            Self::NotFound404 => StatusCode::NOT_FOUND,
        }
    }

    /// Human-readable message for the deny response body. Kept
    /// generic so a `404`-mode response does not accidentally
    /// reveal existence via the message text.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Forbidden403 => "forbidden",
            Self::NotFound404 => "not found",
        }
    }

    /// Build the [`AutumnError`](crate::AutumnError) used by the
    /// `#[authorize]` macro and `#[repository]`-generated
    /// endpoints when a policy denies an action.
    #[must_use]
    pub fn into_error(self) -> crate::AutumnError {
        crate::AutumnError::from(std::io::Error::other(self.message()))
            .with_status(self.status())
    }
}

impl std::str::FromStr for ForbiddenResponse {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "403" | "forbidden" | "Forbidden" => Ok(Self::Forbidden403),
            "404" | "not_found" | "NotFound" | "" => Ok(Self::NotFound404),
            other => Err(format!(
                "invalid forbidden_response: {other:?} (expected \"403\" or \"404\")"
            )),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ForbiddenResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

// ── Runtime authorization helpers ───────────────────────────────

/// Resolve the registered [`Policy`] for resource `R`, run the
/// named action, and return the configured deny response on
/// failure.
///
/// This is the workhorse called by the `#[authorize]` attribute
/// macro and by `#[repository(policy = ...)]`-generated handlers.
/// Hand-written handlers can call it directly to short-circuit a
/// route after loading the resource — the inline pattern that
/// replaces the hand-rolled `if record.author_id != user_id { ... }`
/// snippets the reddit-clone migration removes.
///
/// # Errors
///
/// Returns the configured [`ForbiddenResponse`] error when the
/// policy denies the action, or a `500` when no policy is
/// registered for `R`.
///
/// # Examples
///
/// ```rust,ignore
/// use autumn_web::prelude::*;
/// use autumn_web::authorization::authorize;
///
/// async fn delete_post(
///     state: AppState,
///     session: Session,
///     mut db: Db,
///     post: Post,
/// ) -> AutumnResult<()> {
///     authorize::<Post>(&state, &session, "delete", &post).await?;
///     // ... actually delete
///     Ok(())
/// }
/// ```
pub async fn authorize<R>(
    state: &crate::AppState,
    session: &Session,
    action: &str,
    resource: &R,
) -> crate::AutumnResult<()>
where
    R: Send + Sync + 'static,
{
    let registry = state.policy_registry();
    let policy = registry.policy::<R>().ok_or_else(|| {
        crate::AutumnError::from(std::io::Error::other(format!(
            "no policy registered for resource type {}",
            std::any::type_name::<R>()
        )))
        .with_status(StatusCode::INTERNAL_SERVER_ERROR)
    })?;

    let auth_key = state.auth_session_key();
    let mut ctx = PolicyContext::from_session(session, auth_key).await;
    #[cfg(feature = "db")]
    {
        if let Some(pool) = state.pool() {
            ctx.pool = Some(pool.clone());
        }
    }

    if policy.can(action, &ctx, resource).await {
        Ok(())
    } else {
        Err(state.forbidden_response().into_error())
    }
}

/// Internal alias used by the `#[authorize]` proc-macro and the
/// `#[repository(policy = ...)]` generated handlers. **Not part of
/// the public API** — call [`authorize`] from user code.
#[doc(hidden)]
pub async fn __check_policy<R>(
    state: &crate::AppState,
    session: &Session,
    action: &str,
    resource: &R,
) -> crate::AutumnResult<()>
where
    R: Send + Sync + 'static,
{
    authorize(state, session, action, resource).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone, PartialEq)]
    struct Note {
        author_id: i64,
    }

    #[derive(Default)]
    struct AdminOrOwnerPolicy;

    impl Policy<Note> for AdminOrOwnerPolicy {
        fn can_show<'a>(
            &'a self,
            _ctx: &'a PolicyContext,
            _note: &'a Note,
        ) -> BoxFuture<'a, bool> {
            Box::pin(async { true })
        }
        fn can_update<'a>(
            &'a self,
            ctx: &'a PolicyContext,
            note: &'a Note,
        ) -> BoxFuture<'a, bool> {
            Box::pin(async move {
                ctx.has_role("admin") || ctx.user_id_i64() == Some(note.author_id)
            })
        }
        fn can_delete<'a>(
            &'a self,
            ctx: &'a PolicyContext,
            _note: &'a Note,
        ) -> BoxFuture<'a, bool> {
            Box::pin(async move { ctx.has_role("admin") })
        }
    }

    fn ctx(user_id: Option<&str>, role: Option<&str>) -> PolicyContext {
        let session = Session::new_for_test(String::new(), HashMap::new());
        PolicyContext {
            session,
            user_id: user_id.map(str::to_owned),
            roles: role.into_iter().map(str::to_owned).collect(),
            #[cfg(feature = "db")]
            pool: None,
        }
    }

    #[tokio::test]
    async fn default_impls_deny() {
        struct EmptyPolicy;
        impl Policy<Note> for EmptyPolicy {}
        let policy = EmptyPolicy;
        let c = ctx(Some("1"), None);
        let n = Note { author_id: 1 };
        assert!(!policy.can_show(&c, &n).await);
        assert!(!policy.can_create(&c, &n).await);
        assert!(!policy.can_update(&c, &n).await);
        assert!(!policy.can_delete(&c, &n).await);
        assert!(!policy.can("publish", &c, &n).await);
    }

    #[tokio::test]
    async fn owner_can_update() {
        let policy = AdminOrOwnerPolicy;
        let c = ctx(Some("42"), None);
        let n = Note { author_id: 42 };
        assert!(policy.can_update(&c, &n).await);
        assert!(!policy.can_delete(&c, &n).await);
    }

    #[tokio::test]
    async fn non_owner_cannot_update() {
        let policy = AdminOrOwnerPolicy;
        let c = ctx(Some("99"), None);
        let n = Note { author_id: 42 };
        assert!(!policy.can_update(&c, &n).await);
    }

    #[tokio::test]
    async fn admin_can_delete() {
        let policy = AdminOrOwnerPolicy;
        let c = ctx(Some("99"), Some("admin"));
        let n = Note { author_id: 42 };
        assert!(policy.can_delete(&c, &n).await);
    }

    #[tokio::test]
    async fn can_dispatches_named_actions() {
        let policy = AdminOrOwnerPolicy;
        let c = ctx(Some("42"), None);
        let n = Note { author_id: 42 };
        assert!(policy.can("show", &c, &n).await);
        assert!(policy.can("update", &c, &n).await);
        assert!(policy.can("edit", &c, &n).await);
        assert!(!policy.can("publish", &c, &n).await);
    }

    #[test]
    fn policy_registry_stores_and_resolves() {
        let registry = PolicyRegistry::default();
        registry.register_policy::<Note, _>(AdminOrOwnerPolicy);
        assert!(registry.has_policy::<Note>());
        assert!(registry.policy::<Note>().is_some());
        assert!(registry.scope::<Note>().is_none());
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn double_policy_registration_panics() {
        let registry = PolicyRegistry::default();
        registry.register_policy::<Note, _>(AdminOrOwnerPolicy);
        registry.register_policy::<Note, _>(AdminOrOwnerPolicy);
    }

    #[test]
    fn forbidden_response_default_is_404() {
        let resp = ForbiddenResponse::default();
        assert_eq!(resp, ForbiddenResponse::NotFound404);
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn forbidden_response_parses_strings() {
        assert_eq!(
            "403".parse::<ForbiddenResponse>().unwrap(),
            ForbiddenResponse::Forbidden403
        );
        assert_eq!(
            "404".parse::<ForbiddenResponse>().unwrap(),
            ForbiddenResponse::NotFound404
        );
        assert_eq!(
            "forbidden".parse::<ForbiddenResponse>().unwrap(),
            ForbiddenResponse::Forbidden403
        );
        assert!("418".parse::<ForbiddenResponse>().is_err());
    }

    #[test]
    fn policy_context_helpers() {
        let c = ctx(Some("42"), Some("editor"));
        assert!(c.is_authenticated());
        assert_eq!(c.user_id_i64(), Some(42));
        assert!(c.has_role("editor"));
        assert!(!c.has_role("admin"));
        assert!(c.has_any_role(["admin", "editor"]));
        assert!(!c.has_any_role(["viewer", "guest"]));
    }
}
