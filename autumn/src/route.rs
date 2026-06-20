//! Route descriptor types used by macro-generated code.
//!
//! Each route macro ([`get`](crate::get), [`post`](crate::post), etc.)
//! generates a companion function that returns a [`Route`]. The
//! [`routes!`](crate::routes) macro collects these into a `Vec<Route>`
//! for the [`AppBuilder`](crate::app::AppBuilder).
//!
//! Users do not construct `Route` values directly -- they use the
//! proc macros and the `routes![]` collection macro.

use std::time::Duration;

use axum::routing::MethodRouter;
use http::Method;

use crate::openapi::ApiDoc;
use crate::state::AppState;

/// Metadata attached to routes emitted by the `#[repository(api = ...)]` macro.
///
/// Lets the app builder validate, at startup, that every auto-mounted CRUD
/// endpoint is paired with a registered
/// [`Policy`](crate::authorization::Policy).
#[derive(Debug, Clone, Copy)]
pub struct RepositoryApiMeta {
    /// Stringified resource type name (e.g., `"Post"`). Used for
    /// log messages and to look up the registered policy via
    /// [`std::any::TypeId`] indirectly through the generated check
    /// function in [`Self::policy_check`].
    pub resource_type_name: &'static str,

    /// Path prefix mounted by this repository (e.g., `"/api/posts"`).
    pub api_path: &'static str,

    /// `true` when the macro form used `policy = SomePolicy`, so the
    /// auto-generated handlers enforce a record-level check before
    /// running. `false` when the macro form is just
    /// `#[repository(api = "...")]` — that form is rejected in
    /// `prod` profile builds unless
    /// `[security] allow_unauthorized_repository_api = true`.
    pub has_policy: bool,

    /// Type-erased registry probe emitted by the macro when
    /// `policy = ...` is set. Returns `true` if a [`Policy`](crate::authorization::Policy) is
    /// registered on the runtime
    /// [`PolicyRegistry`](crate::authorization::PolicyRegistry) for
    /// the resource type. Lets the app builder fail fast at
    /// startup when a developer wires `policy = X` on the
    /// `#[repository]` macro but forgets to call
    /// `.policy::<R, _>(X)` on the builder — without this check,
    /// every protected request would 500 with "no policy
    /// registered" instead of failing fast at boot. `None` when
    /// the macro form omits `policy = ...`.
    pub policy_check: Option<fn(&crate::authorization::PolicyRegistry) -> bool>,

    /// Type-erased registry probe emitted by the macro when
    /// `scope = ...` is set. Returns `true` if a [`Scope`](crate::authorization::Scope) is
    /// registered for the resource type. Companion to
    /// [`Self::policy_check`] for the scope-list code path: the
    /// generated `GET /<api>` handler resolves the scope from the
    /// registry on every request, so a missing
    /// `.scope::<R, _>(...)` registration would 500 every list
    /// call. The startup guard fails fast instead. `None` when
    /// the macro form omits `scope = ...`.
    pub scope_check: Option<fn(&crate::authorization::PolicyRegistry) -> bool>,
}

/// Declares how the app-level idempotency layer should replay cached responses
/// for this route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteIdempotency {
    /// Unknown/manual routes have no guaranteed generated replay consumer.
    /// Autumn stores the first successful mutation but fails closed on cache
    /// hits instead of directly replaying a stale success around any
    /// route-local authorization, tenant, audit, or similar layers.
    #[default]
    Direct,
    /// Autumn-generated routes install a replay consumer inside the route
    /// stack or generated guard body, allowing route-local middleware and
    /// guards to run before the cached response is returned.
    ///
    /// Manual layered routes can use this too, but they must place
    /// [`crate::idempotency::IdempotencyReplayLayer`] after those checks and
    /// before the mutating handler.
    ReplayThroughInner,
}

/// Per-route override for the global inbound request timeout
/// (`[server.timeouts] request_timeout_ms`).
///
/// Emitted by the route macros from the `timeout_ms = ...` / `timeout = "off"`
/// attributes and consulted by the timeout middleware (keyed by the matched
/// route template). The default, [`RouteTimeout::Inherit`], applies the global
/// deadline. Long-lived primitives like WebSocket upgrades default to
/// [`RouteTimeout::Disabled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteTimeout {
    /// Use the global `request_timeout_ms` deadline (or none if disabled).
    #[default]
    Inherit,
    /// Override the global deadline with a route-specific wall-clock budget,
    /// for known-slow endpoints (report exports, large uploads).
    Override(Duration),
    /// Exempt this route from the global deadline entirely (SSE, WebSocket,
    /// long-poll, and other intentionally long-lived routes).
    Disabled,
}

/// A single route binding an HTTP method + path to an Axum handler.
///
/// Created by the `__autumn_route_info_{name}()` companion functions
/// that route macros ([`get`](crate::get), [`post`](crate::post), etc.)
/// generate. Users don't construct this directly -- they use the
/// attribute macros and the [`routes!`](crate::routes) macro.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/hello")]
/// async fn hello() -> &'static str { "hi" }
///
/// // `routes!` expands to a Vec<Route>:
/// let route_vec: Vec<autumn_web::Route> = routes![hello];
/// assert_eq!(route_vec.len(), 1);
/// ```
pub struct Route {
    /// HTTP method (`GET`, `POST`, `PUT`, `DELETE`, etc.).
    pub method: Method,

    /// URL path pattern (e.g., `"/users/{id}"`).
    pub path: &'static str,

    /// Axum [`MethodRouter`] that handles requests matching this route.
    pub handler: MethodRouter<AppState>,

    /// Handler function name, used for startup logging
    /// (e.g., `"hello"`, `"create_item"`).
    pub name: &'static str,

    /// `OpenAPI` metadata inferred from the handler's signature and any
    /// [`#[api_doc(...)]`](crate::api_doc) overrides. Consumed by
    /// `AppBuilder::openapi` when
    /// generating `/v3/api-docs`.
    pub api_doc: ApiDoc,

    /// API version of the route (e.g. "v1")
    pub api_version: Option<&'static str>,

    /// Whether this route opts out of sunset 410 response
    pub sunset_opt_out: bool,

    /// Repository auto-API metadata, populated by the
    /// `#[repository(api = ...)]` macro. `None` for hand-written
    /// route handlers.
    pub repository: Option<RepositoryApiMeta>,

    /// Idempotency replay behavior for this route.
    pub idempotency: RouteIdempotency,

    /// Per-route override for the global inbound request timeout.
    pub timeout: RouteTimeout,
}
