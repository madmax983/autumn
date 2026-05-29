//! Router construction and configuration.
//!
//! This module handles assembling the final [`axum::Router`] from the various
//! components configured in [`AppBuilder`](crate::app::AppBuilder), including
//! user routes, static files, middleware, error pages, and framework endpoints
//! like actuators and probes.

use std::sync::Arc;
use std::time::Duration;

use crate::config::AutumnConfig;
use crate::error_pages::{self, SharedRenderer};
use crate::extract::State;
use crate::idempotency::{IdempotencyLayer, IdempotencyStore, MemoryIdempotencyStore};
use crate::middleware::RequestIdLayer;
use crate::middleware::dev;
use crate::middleware::exception_filter::{
    ExceptionFilter, ExceptionFilterLayer, ProblemDetailsFilter,
};
use crate::route::Route;
use crate::route::ScopedGroup;
use crate::state::AppState;
use axum::middleware::Next;
use axum::response::IntoResponse;
use http::{Request, StatusCode};
use thiserror::Error;

pub const DEFAULT_FAVICON_PATH: &str = "/favicon.ico";

/// Errors that can occur during the router build process.
///
/// These errors are typically fatal and represent configuration or routing
/// definition issues that must be fixed before the application can start.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RouterBuildError {
    /// The session backend configuration is invalid (e.g. Redis without a URL).
    #[error("invalid session backend configuration: {0}")]
    InvalidSessionBackend(#[from] crate::session::SessionBackendConfigError),
    /// The idempotency backend configuration is invalid.
    #[error("invalid idempotency backend configuration: {0}")]
    #[allow(dead_code)] // constructed only in the `redis` feature path
    InvalidIdempotencyBackend(String),
    /// A user-defined route conflicts with a framework-provided route.
    #[error("framework route overlap at {path}: {existing} conflicts with {incoming}")]
    FrameworkRouteOverlap {
        /// The HTTP path where the overlap occurred.
        path: String,
        /// The name of the existing framework route.
        existing: &'static str,
        /// The name of the incoming user route.
        incoming: &'static str,
    },
    /// An `OpenApiConfig` path (e.g. `openapi_json_path` or
    /// `swagger_ui_path`) is not a valid route path (must start with `/`
    /// and be non-empty).
    #[cfg(feature = "openapi")]
    #[error("invalid OpenAPI {field} path: {value:?} (must start with '/' and be non-empty)")]
    InvalidOpenApiPath {
        /// Which config field carried the invalid path.
        field: &'static str,
        /// The offending value from the user's config.
        value: String,
    },
    /// `openapi_json_path` and `swagger_ui_path` collide on the same
    /// URL. Mounting both would cause axum to panic on overlapping
    /// method routes at startup.
    #[cfg(feature = "openapi")]
    #[error(
        "openapi_json_path and swagger_ui_path both resolve to {path:?}; they must differ or `swagger_ui_path` must be `None`"
    )]
    DuplicateOpenApiPath {
        /// The path that both fields pointed at.
        path: String,
    },
    /// An `OpenAPI` mount path overlaps with an existing `GET` handler,
    /// which would panic at `axum::Router::merge` time.
    #[cfg(feature = "openapi")]
    #[error(
        "OpenAPI {field} path {path:?} collides with an existing GET route; choose a different `OpenApiConfig::{field}`"
    )]
    OpenApiPathCollision {
        /// Which config field carried the colliding path.
        field: &'static str,
        /// The colliding path.
        path: String,
    },
}

/// Build the fully-configured Axum router from routes, config, and state.
///
/// Extracted from `AppBuilder::run` so the router construction logic is
/// testable without binding a real TCP listener.
///
/// # Panics
///
/// Panics when framework router assembly encounters invalid configuration.
/// Use [`try_build_router`] to handle configuration errors explicitly.
#[allow(dead_code)]
pub fn build_router(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
) -> axum::Router {
    try_build_router(route_list, config, state)
        .unwrap_or_else(|error| panic!("invalid router configuration: {error}"))
}

/// Checked variant of [`build_router`] that returns configuration errors
/// instead of panicking.
///
/// # Errors
///
/// Returns [`RouterBuildError`] when router assembly encounters invalid
/// framework configuration, such as an unusable session backend.
pub struct RouterContext {
    pub exception_filters: Vec<Arc<dyn ExceptionFilter>>,
    pub scoped_groups: Vec<ScopedGroup>,
    pub merge_routers: Vec<axum::Router<AppState>>,
    pub nest_routers: Vec<(String, axum::Router<AppState>)>,
    /// Custom Tower layers registered via
    /// [`AppBuilder::layer`](crate::app::AppBuilder::layer). Applied inside
    /// [`RequestIdLayer`] and the session layer on the ingress path so user
    /// middleware observes the generated request ID and session context.
    ///
    /// **SSG/ISG mode trade-off**: when `dist_dir` is active, layers are
    /// moved outside the static-first middleware so they can process
    /// pre-rendered responses (e.g. compression).  As a side effect they also
    /// run *before* `RequestIdLayer`, session, `MetricsLayer`, and
    /// `ExceptionFilterLayer` for all requests (static and dynamic).  Layers
    /// that depend on extensions set by those framework layers — such as the
    /// request ID or session data — will not find them in SSG mode.
    pub custom_layers: Vec<crate::app::CustomLayerRegistration>,
    pub error_page_renderer: Option<SharedRenderer>,
    /// Custom session store installed via
    /// [`AppBuilder::with_session_store`](crate::app::AppBuilder::with_session_store).
    /// When `Some`, [`apply_session_layer`](crate::session::apply_session_layer)
    /// uses it directly and skips the config-driven backend selection.
    pub session_store: Option<Arc<dyn crate::session::BoxedSessionStore>>,
    /// `OpenAPI` generation configuration. When `Some`, the router mounts
    /// an `openapi.json` endpoint and (optionally) a Swagger UI page
    /// describing the application's routes.
    ///
    /// Gated behind the `openapi` feature.
    #[cfg(feature = "openapi")]
    pub openapi: Option<crate::openapi::OpenApiConfig>,
}

/// Checked variant of [`build_router`] that returns configuration errors
/// instead of panicking.
///
/// # Errors
///
/// Returns [`RouterBuildError`] when router assembly encounters invalid
/// framework configuration, such as an unusable session backend.
pub fn try_build_router(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
) -> Result<axum::Router, RouterBuildError> {
    let startup_barrier_state = state.clone();
    let router = try_build_router_inner(
        route_list,
        config,
        state,
        RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            #[cfg(feature = "openapi")]
            openapi: None,
        },
    )?;
    Ok(apply_startup_barrier(
        router,
        config,
        &startup_barrier_state,
    ))
}

/// Build a router that includes user-supplied raw Axum routers.
///
/// Like [`build_router`], but also merges and nests additional raw
/// Axum routers. This is primarily useful for integration testing;
/// in production, use [`AppBuilder::merge`](crate::app::AppBuilder::merge) and [`AppBuilder::nest`](crate::app::AppBuilder::nest).
///
/// # Panics
///
/// Panics when framework router assembly encounters invalid configuration.
/// Use [`try_build_router_merged`] to handle configuration errors explicitly.
#[allow(dead_code)]
pub fn build_router_merged(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
) -> axum::Router {
    try_build_router_merged(route_list, config, state, merge_routers, nest_routers)
        .unwrap_or_else(|error| panic!("invalid router configuration: {error}"))
}

/// Checked variant of [`build_router_merged`] that returns configuration
/// errors instead of panicking.
///
/// # Errors
///
/// Returns [`RouterBuildError`] when router assembly encounters invalid
/// framework configuration, such as an unusable session backend.
#[allow(dead_code)]
pub fn try_build_router_merged(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
) -> Result<axum::Router, RouterBuildError> {
    let startup_barrier_state = state.clone();
    let router = try_build_router_inner(
        route_list,
        config,
        state,
        RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers,
            nest_routers,
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            #[cfg(feature = "openapi")]
            openapi: None,
        },
    )?;
    Ok(apply_startup_barrier(
        router,
        config,
        &startup_barrier_state,
    ))
}

pub fn try_build_router_inner(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    ctx: RouterContext,
) -> Result<axum::Router, RouterBuildError> {
    let router = build_router_pre_state(route_list, config, &state, ctx, None)?;
    Ok(router.with_state(state))
}

/// Like [`try_build_router_inner`] but returns `Router<AppState>` before
/// [`with_state`](axum::Router::with_state) is called.  Used by
/// [`try_build_router_with_static_inner`] so that user layers and the static
/// file middleware can be applied to the typed router before state is baked in.
fn build_router_pre_state(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: &AppState,
    ctx: RouterContext,
    // When custom_layers are extracted from ctx before this call (SSG path),
    // the caller pre-computes the flag so the idempotency selector still sees
    // the real layer list even though ctx.custom_layers is empty.
    opaque_app_layers_override: Option<bool>,
) -> Result<axum::Router<AppState>, RouterBuildError> {
    // Fail-fast if an OpenAPI mount path collides with a user or
    // framework GET route — axum panics on overlapping method routes,
    // so surface this as a recoverable error before we start merging.
    #[cfg(feature = "openapi")]
    reject_openapi_path_collisions(
        ctx.openapi.as_ref(),
        &route_list,
        &ctx.scoped_groups,
        &ctx.merge_routers,
        &ctx.nest_routers,
        config,
    )?;

    // Build the OpenAPI spec BEFORE moving the routes into axum, because
    // group_and_mount_routes consumes the Route list.
    #[cfg(feature = "openapi")]
    let openapi_router = build_openapi_router(
        &route_list,
        &ctx.scoped_groups,
        ctx.openapi.as_ref(),
        &config.session.cookie_name,
    )?;

    let idempotency_layers = build_idempotency_layers(config, state)?;
    let opaque_app_layers_present = opaque_app_layers_override
        .unwrap_or_else(|| custom_layers_require_fail_closed_idempotency(&ctx.custom_layers));
    let mut router = group_and_mount_routes(
        route_list,
        idempotency_layers.as_ref(),
        opaque_app_layers_present,
    );

    let dev_reload_enabled = dev::is_enabled_with_env(&crate::config::OsEnv);

    router = mount_framework_routes(router, config, dev_reload_enabled);

    let (mounted_probe_paths, router_with_probes) = mount_probe_endpoints(router, config);
    router = router_with_probes;

    router = mount_actuator_endpoints(router, config, &mounted_probe_paths)?;

    #[cfg(feature = "openapi")]
    if let Some(openapi_router) = openapi_router {
        router = router.merge(openapi_router);
    }

    // Static file serving from project's static/ directory.
    // Fingerprinted assets (e.g. `autumn.a1b2c3d4.css`) are served with
    // `Cache-Control: public, max-age=31536000, immutable`; all other static
    // files use the default browser policy.
    let env = crate::config::OsEnv;
    let static_dir = crate::app::project_dir("static", &env);
    router = router.nest_service("/static", tower_http::services::ServeDir::new(&static_dir));
    router = router.layer(axum::middleware::from_fn(asset_cache_control));

    router = mount_scoped_groups(router, ctx.scoped_groups, idempotency_layers.as_ref());

    router = mount_raw_routers(
        router,
        ctx.merge_routers,
        ctx.nest_routers,
        idempotency_layers.as_ref(),
    );

    router = apply_middleware(
        router,
        config,
        state,
        ctx.exception_filters,
        ctx.custom_layers,
        ctx.error_page_renderer,
        ctx.session_store,
    )?;

    if dev_reload_enabled {
        router = router
            .layer(axum::middleware::from_fn(dev::disable_static_cache))
            .layer(axum::middleware::from_fn(dev::inject_live_reload));
    }

    #[cfg(feature = "oauth2")]
    let router = router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        http_interceptor_middleware,
    ));

    Ok(router)
}

/// Parse `{name}` captures from a route path.
///
/// Mirrors the compile-time extractor in `autumn_macros::api_doc` so
/// runtime spec assembly (which sees scope prefixes that the macro
/// never does) produces consistent parameter lists.
#[cfg(feature = "openapi")]
pub fn extract_path_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut remaining = path;

    while let Some(start) = remaining.find('{') {
        let after_brace = &remaining[start + 1..];
        let Some(end_rel) = after_brace.find('}') else {
            break;
        };

        let inner = &after_brace[..end_rel];
        let name = inner.split(':').next().unwrap_or(inner).trim();
        if !name.is_empty() {
            out.push(name.to_owned());
        }

        remaining = &after_brace[end_rel + 1..];
    }

    out
}

/// Build an Axum sub-router that serves the generated `OpenAPI` document
/// and (optionally) a Swagger UI HTML page.
///
/// Returns `None` when `OpenAPI` generation is disabled, i.e. the user
/// never called [`AppBuilder::openapi`](crate::app::AppBuilder::openapi).
///
/// The spec is rendered once at build time and stored in an `Arc<String>`
/// so the `/v3/api-docs` handler performs no serialization per request.
#[cfg(feature = "openapi")]
fn build_openapi_router(
    route_list: &[Route],
    scoped_groups: &[ScopedGroup],
    openapi_config: Option<&crate::openapi::OpenApiConfig>,
    session_cookie_name: &str,
) -> Result<Option<axum::Router<AppState>>, RouterBuildError> {
    let Some(config) = openapi_config else {
        return Ok(None);
    };
    let mut config = config.clone();
    session_cookie_name.clone_into(&mut config.session_cookie_name);

    // Validate user-provided paths up front so a typo like
    // `"openapi.json"` surfaces as a recoverable RouterBuildError
    // rather than an axum panic (`Paths must start with a '/'`).
    validate_route_path("openapi_json_path", &config.openapi_json_path)?;
    if let Some(path) = &config.swagger_ui_path {
        validate_route_path("swagger_ui_path", path)?;
        // Registering two GET handlers on the same path would cause an
        // axum `Route::route` panic, so reject collisions as a
        // configuration error instead.
        if path == &config.openapi_json_path {
            return Err(RouterBuildError::DuplicateOpenApiPath { path: path.clone() });
        }
    }

    let docs = collect_openapi_docs(route_list, scoped_groups);

    let refs: Vec<&crate::openapi::ApiDoc> = docs.iter().collect();
    let spec = crate::openapi::generate_spec(&config, &refs);
    let spec_json = serde_json::to_string_pretty(&spec)
        .unwrap_or_else(|e| format!("{{\"error\": \"failed to serialize spec: {e}\"}}"));

    let spec_body = Arc::new(spec_json);
    let json_path = config.openapi_json_path.clone();
    let swagger_path = config.swagger_ui_path.clone();
    let title = config.title.clone();

    let mut router = axum::Router::<AppState>::new().route(
        &json_path,
        axum::routing::get(move || {
            let spec = spec_body.clone();
            async move {
                use axum::response::IntoResponse;
                (
                    [(http::header::CONTENT_TYPE, "application/json")],
                    (*spec).clone(),
                )
                    .into_response()
            }
        }),
    );

    if let Some(path) = swagger_path {
        router = mount_swagger_ui_routes(router, &path, &title, &json_path);
    }

    tracing::debug!(
        openapi_json = %json_path,
        swagger_ui = ?config.swagger_ui_path,
        swagger_ui_version = crate::openapi::SWAGGER_UI_VERSION,
        "Mounted OpenAPI endpoints"
    );

    Ok(Some(router))
}

/// Join a nest/scope prefix with a child route path, matching
/// `axum::Router::nest` normalization.
///
/// `nest("/api", r)` mounts r's `/` at `/api` (not `/api/`), and any
/// other child path `/foo` at `/api/foo`. The collision check and the
/// path emitted into the `OpenAPI` spec must use the same shape or we
/// end up either missing real collisions (the reviewer's case:
/// `/api` + `/` recorded as `/api/` but axum routes it at `/api`) or
/// generating a spec whose URLs don't match what axum serves.
#[cfg(feature = "openapi")]
pub fn join_nested_path(prefix: &str, child: &str) -> String {
    let prefix_trimmed = prefix.trim_end_matches('/');
    if child == "/" || child.is_empty() {
        if prefix_trimmed.is_empty() {
            "/".to_owned()
        } else {
            prefix_trimmed.to_owned()
        }
    } else if child.starts_with('/') {
        format!("{prefix_trimmed}{child}")
    } else {
        format!("{prefix_trimmed}/{child}")
    }
}

/// Shared validator for user-supplied `OpenAPI` mount paths.
///
/// Catches the common typos that would otherwise manifest as axum
/// panics inside `Router::route` at startup:
///
/// * empty or missing leading slash,
/// * unbalanced `{` / `}` pairs,
/// * any `{…}` / `{*…}` capture or wildcard syntax (the mount points
///   are static endpoints — a user that needs templated paths shouldn't
///   be using this field), and
/// * any `*` wildcard character (axum treats these as catch-alls).
///
/// The check intentionally stays conservative: rejecting a few valid-
/// but-weird paths is far better than letting a typo like
/// `"openapi.json"` or `"/docs/{id}"` crash boot.
#[cfg(feature = "openapi")]
fn validate_route_path(field: &'static str, value: &str) -> Result<(), RouterBuildError> {
    let reject = |reason_fragment: &str| {
        Err(RouterBuildError::InvalidOpenApiPath {
            field,
            value: format!("{value:?} {reason_fragment}"),
        })
    };

    if value.is_empty() {
        return reject("(must be non-empty)");
    }
    if !value.starts_with('/') {
        return reject("(must start with '/')");
    }
    // Double-slash inside the path is almost always a typo (e.g.
    // `//v3/api-docs`) and axum normalizes it away on match, so
    // treating it as invalid avoids surprising "route can't be hit"
    // reports in the field.
    if value.contains("//") {
        return reject("(must not contain '//')");
    }

    let mut depth: i32 = 0;
    for ch in value.chars() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return reject("(unbalanced '}')");
                }
            }
            '*' => return reject("(wildcard '*' is not allowed in an OpenAPI mount path)"),
            _ => {}
        }
    }
    if depth != 0 {
        return reject("(unbalanced '{')");
    }
    if value.contains('{') {
        return reject("(OpenAPI mount paths must be static; `{…}` captures are not allowed)");
    }
    Ok(())
}

/// Reject `OpenAPI` mount paths that overlap with an existing `GET`
/// handler.
///
/// `axum::Router::merge` panics when the merged routers have method
/// handlers on the same path (e.g. two `GET` handlers on
/// `/v3/api-docs`). We surface that as a recoverable
/// [`RouterBuildError::OpenApiPathCollision`] so misconfiguration
/// produces an actionable error instead of a crash on startup.
///
/// We check against:
/// * user routes (top-level + scoped groups) that will be mounted
///   before the `OpenAPI` sub-router merges in,
/// * framework `GET`s: probes, actuator, htmx assets, and dev
///   live-reload when enabled,
/// * nest prefixes from [`AppBuilder::nest`](crate::app::AppBuilder::nest)
///   when the `OpenAPI` path falls under one.
///
/// Raw routers passed to [`AppBuilder::merge`](crate::app::AppBuilder::merge)
/// cannot be introspected — axum does not expose their route table.
/// We emit a `tracing::warn!` so operators know the check is
/// incomplete in that case.
#[cfg(feature = "openapi")]
fn reject_openapi_path_collisions(
    openapi_config: Option<&crate::openapi::OpenApiConfig>,
    route_list: &[Route],
    scoped_groups: &[ScopedGroup],
    merge_routers: &[axum::Router<AppState>],
    nest_routers: &[(String, axum::Router<AppState>)],
    config: &AutumnConfig,
) -> Result<(), RouterBuildError> {
    let Some(openapi) = openapi_config else {
        return Ok(());
    };

    // Gather every path a GET (or WS, which mounts as GET) will already
    // own by the time we merge.
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for route in route_list {
        if route.method == http::Method::GET || route.method.as_str() == "WS" {
            claimed.insert(route.path.to_owned());
        }
    }
    for group in scoped_groups {
        for route in &group.routes {
            if route.method == http::Method::GET || route.method.as_str() == "WS" {
                claimed.insert(join_nested_path(&group.prefix, route.path));
            }
        }
    }
    // Framework-mounted GETs.
    claimed.insert(config.health.path.clone());
    claimed.insert(config.health.live_path.clone());
    claimed.insert(config.health.ready_path.clone());
    claimed.insert(config.health.startup_path.clone());
    for path in
        crate::actuator::actuator_endpoint_paths(&config.actuator.prefix, config.actuator.sensitive)
    {
        claimed.insert(path);
    }
    #[cfg(feature = "htmx")]
    {
        claimed.insert(crate::htmx::HTMX_JS_PATH.to_owned());
        claimed.insert(crate::htmx::HTMX_CSRF_JS_PATH.to_owned());
    }
    // Dev live-reload endpoints are only mounted when the env vars
    // that enable them are set, but reserving the paths regardless
    // makes the error message deterministic across dev/prod.
    if dev::is_enabled_with_env(&crate::config::OsEnv) {
        claimed.insert(dev::LIVE_RELOAD_PATH.to_owned());
        claimed.insert(dev::LIVE_RELOAD_SCRIPT_PATH.to_owned());
    }
    #[cfg(feature = "mail")]
    if config
        .mail
        .preview_routes_enabled(config.profile.as_deref())
    {
        claimed.insert(crate::mail::MAIL_PREVIEW_PATH.to_owned());
        claimed.insert("/_autumn/mail/messages/{message_id}".to_owned());
        claimed.insert("/_autumn/mail/previews/{mailer}/{method}".to_owned());
    }

    check_openapi_path_against(
        "openapi_json_path",
        &openapi.openapi_json_path,
        &claimed,
        nest_routers,
    )?;
    if let Some(path) = &openapi.swagger_ui_path {
        check_openapi_path_against("swagger_ui_path", path, &claimed, nest_routers)?;
        let mut claimed_with_openapi = claimed.clone();
        claimed_with_openapi.insert(openapi.openapi_json_path.clone());
        for asset_path in crate::openapi::swagger_ui_asset_paths(path) {
            check_openapi_path_against(
                "swagger_ui_path",
                &asset_path,
                &claimed_with_openapi,
                nest_routers,
            )?;
        }
    }

    // Raw merged routers are opaque — we can't inspect their route
    // tables through the axum API. Warn instead of failing so users
    // know the check doesn't cover this code path.
    if !merge_routers.is_empty() {
        tracing::warn!(
            openapi_json_path = %openapi.openapi_json_path,
            swagger_ui_path = ?openapi.swagger_ui_path,
            merged_routers = merge_routers.len(),
            "OpenAPI mount collision check skipped for AppBuilder::merge routers: \
             axum does not expose their route table, so overlapping GET handlers \
             will still panic at startup. Choose OpenAPI paths that don't overlap \
             with any merged router's handlers."
        );
    }

    Ok(())
}

/// Evaluate a single `OpenAPI` path against the claimed-path set plus
/// any nest prefixes. Returns an `OpenApiPathCollision` error on
/// collision.
#[cfg(feature = "openapi")]
fn check_openapi_path_against(
    field: &'static str,
    path: &str,
    claimed: &std::collections::HashSet<String>,
    nest_routers: &[(String, axum::Router<AppState>)],
) -> Result<(), RouterBuildError> {
    if claimed.contains(path) {
        return Err(RouterBuildError::OpenApiPathCollision {
            field,
            path: path.to_owned(),
        });
    }
    // A nest prefix P owns every route under P (`/P/...`), so any
    // OpenAPI path that equals P or starts with `P/` will either
    // panic on merge (exact match) or nest inside the user's router
    // (where axum routing semantics decide which handler wins).
    // Reject both cases so the spec endpoint can't silently vanish.
    for (prefix, _) in nest_routers {
        let prefix_slash = format!("{prefix}/");
        if path == prefix || path.starts_with(&prefix_slash) {
            return Err(RouterBuildError::OpenApiPathCollision {
                field,
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}

fn group_and_mount_routes(
    route_list: Vec<Route>,
    idempotency_layers: Option<&BuiltIdempotencyLayers>,
    opaque_app_layers_present: bool,
) -> axum::Router<AppState> {
    // Group routes by path so multiple methods on the same path
    // (e.g. GET /admin + POST /admin) are merged into a single
    // MethodRouter. Axum 0.7+ panics if .route() is called twice
    // with the same path — merging avoids this.
    let mut grouped: indexmap::IndexMap<&str, axum::routing::MethodRouter<AppState>> =
        indexmap::IndexMap::new();
    for route in &route_list {
        tracing::debug!(
            method = %route.method,
            path = route.path,
            name = route.name,
            "Mounted route"
        );
    }
    for route in route_list {
        let selected_layer = idempotency_layers
            .map(|layers| idempotency_layer_for_route(&route, layers, opaque_app_layers_present));
        let mut handler = route.handler;
        if let Some(layer) = selected_layer {
            handler = handler.layer(layer.clone());
        }
        grouped
            .entry(route.path)
            .and_modify(|existing| {
                *existing = std::mem::take(existing).merge(handler.clone());
            })
            .or_insert(handler);
    }

    let mut router = axum::Router::new();
    for (path, method_router) in grouped {
        router = router.route(path, method_router);
    }
    router
}

const fn idempotency_layer_for_route<'a>(
    route: &Route,
    layers: &'a BuiltIdempotencyLayers,
    opaque_app_layers_present: bool,
) -> &'a IdempotencyLayer {
    if opaque_app_layers_present {
        &layers.manual
    } else if route_uses_generated_replay_stop(route) {
        &layers.route
    } else {
        &layers.manual
    }
}

const fn route_uses_generated_replay_stop(route: &Route) -> bool {
    matches!(
        route.idempotency,
        crate::route::RouteIdempotency::ReplayThroughInner
    )
}

fn custom_layers_require_fail_closed_idempotency(
    custom_layers: &[crate::app::CustomLayerRegistration],
) -> bool {
    custom_layers
        .iter()
        .any(|registered| !is_idempotency_transparent_app_layer(registered))
}

fn is_idempotency_transparent_app_layer(registered: &crate::app::CustomLayerRegistration) -> bool {
    registered
        .type_name
        .starts_with("autumn_web::session::SessionLayer<")
        || registered
            .type_name
            .starts_with("autumn::session::SessionLayer<")
        || registered.type_id
            == std::any::TypeId::of::<crate::session::SessionLayer<crate::session::MemoryStore>>()
        || is_i18n_bundle_extension_layer(registered.type_id)
}

#[cfg(feature = "i18n")]
fn is_i18n_bundle_extension_layer(type_id: std::any::TypeId) -> bool {
    type_id == std::any::TypeId::of::<axum::Extension<Arc<crate::i18n::Bundle>>>()
}

#[cfg(not(feature = "i18n"))]
const fn is_i18n_bundle_extension_layer(_type_id: std::any::TypeId) -> bool {
    false
}

#[cfg_attr(not(feature = "mail"), allow(unused_variables))]
#[allow(clippy::cognitive_complexity)]
fn mount_framework_routes(
    mut router: axum::Router<AppState>,
    config: &AutumnConfig,
    dev_reload_enabled: bool,
) -> axum::Router<AppState> {
    #[cfg(not(feature = "mail"))]
    let _ = config;

    // Framework-provided routes
    #[cfg(feature = "htmx")]
    {
        router = router.route(crate::htmx::HTMX_JS_PATH, axum::routing::get(htmx_handler));
        router = router.route(
            crate::htmx::HTMX_CSRF_JS_PATH,
            axum::routing::get(htmx_csrf_handler),
        );
        tracing::debug!(
            method = "GET",
            path = crate::htmx::HTMX_JS_PATH,
            name = format!("htmx {}", crate::htmx::HTMX_VERSION),
            "Mounted route"
        );
        tracing::debug!(
            method = "GET",
            path = crate::htmx::HTMX_CSRF_JS_PATH,
            name = "htmx csrf helper",
            "Mounted route"
        );
    }

    if dev_reload_enabled {
        router = router.route(
            dev::LIVE_RELOAD_PATH,
            axum::routing::get(dev::live_reload_state_handler),
        );
        router = router.route(
            dev::LIVE_RELOAD_SCRIPT_PATH,
            axum::routing::get(dev::live_reload_script_handler),
        );
        tracing::debug!(
            state_path = dev::LIVE_RELOAD_PATH,
            script_path = dev::LIVE_RELOAD_SCRIPT_PATH,
            "Mounted dev live reload endpoints"
        );
    }

    #[cfg(feature = "mail")]
    if config
        .mail
        .preview_routes_enabled(config.profile.as_deref())
    {
        router = router.merge(crate::mail::mail_preview_router(
            config.mail.file_dir.clone(),
        ));
        tracing::debug!(
            path = crate::mail::MAIL_PREVIEW_PATH,
            "Mounted dev mail preview endpoints"
        );
    }

    router
}

fn mount_probe_endpoints<S>(
    mut router: axum::Router<S>,
    config: &AutumnConfig,
) -> (std::collections::HashSet<String>, axum::Router<S>)
where
    S: Clone + Send + Sync + 'static,
    AppState: axum::extract::FromRef<S>,
{
    // Probe endpoints (auto-mounted)
    let mut mounted_probe_paths = std::collections::HashSet::new();

    if mounted_probe_paths.insert(config.health.live_path.clone()) {
        router = router.route(
            &config.health.live_path,
            axum::routing::get(crate::probe::live_handler::<AppState>),
        );
    }
    if mounted_probe_paths.insert(config.health.ready_path.clone()) {
        router = router.route(
            &config.health.ready_path,
            axum::routing::get(crate::probe::ready_handler::<AppState>),
        );
    }
    if mounted_probe_paths.insert(config.health.startup_path.clone()) {
        router = router.route(
            &config.health.startup_path,
            axum::routing::get(crate::probe::startup_handler::<AppState>),
        );
    }
    if mounted_probe_paths.insert(config.health.path.clone()) {
        router = router.route(
            &config.health.path,
            axum::routing::get(crate::health::handler::<AppState>),
        );
    }
    tracing::debug!(
        health = %config.health.path,
        live = %config.health.live_path,
        ready = %config.health.ready_path,
        startup = %config.health.startup_path,
        "Mounted probe endpoints"
    );

    (mounted_probe_paths, router)
}

fn mount_actuator_endpoints(
    mut router: axum::Router<AppState>,
    config: &AutumnConfig,
    mounted_probe_paths: &std::collections::HashSet<String>,
) -> Result<axum::Router<AppState>, RouterBuildError> {
    // Actuator endpoints
    let actuator_sensitive = config.actuator.sensitive;
    let actuator_paths =
        crate::actuator::actuator_endpoint_paths(&config.actuator.prefix, actuator_sensitive);
    if let Some(path) = actuator_paths
        .iter()
        .find(|path| mounted_probe_paths.contains(path.as_str()))
    {
        return Err(RouterBuildError::FrameworkRouteOverlap {
            path: path.clone(),
            existing: "probe endpoint",
            incoming: "actuator endpoint",
        });
    }
    router = router.merge(crate::actuator::actuator_router_with_prefix(
        &config.actuator.prefix,
        actuator_sensitive,
    ));
    tracing::debug!(
        sensitive = actuator_sensitive,
        prefix = %config.actuator.prefix,
        "Mounted actuator endpoints"
    );
    Ok(router)
}

fn mount_scoped_groups(
    mut router: axum::Router<AppState>,
    scoped_groups: Vec<ScopedGroup>,
    idempotency_layers: Option<&BuiltIdempotencyLayers>,
) -> axum::Router<AppState> {
    // Mount scoped route groups (each with its own middleware layer).
    for group in scoped_groups {
        let mut sub_router = axum::Router::new();
        for route in group.routes {
            tracing::debug!(
                method = %route.method,
                path = route.path,
                name = route.name,
                scope = %group.prefix,
                "Mounted scoped route"
            );
            // Scoped groups are wrapped by an opaque user-provided layer after
            // the route handlers are built. The idempotency storage key cannot
            // know whether that layer authorizes, audits, or resolves tenant
            // state from non-whitelisted headers/extensions, so cached hits
            // fail closed instead of replaying through a generated stop inside
            // the scoped route.
            let selected_layer = idempotency_layers.map(|layers| &layers.manual);
            let mut handler = route.handler;
            if let Some(layer) = selected_layer {
                handler = handler.layer(layer.clone());
            }
            sub_router = sub_router.route(route.path, handler);
        }
        sub_router = (group.apply_layer)(sub_router);
        router = router.nest(&group.prefix, sub_router);
    }
    router
}

fn mount_raw_routers(
    mut router: axum::Router<AppState>,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
    idempotency_layers: Option<&BuiltIdempotencyLayers>,
) -> axum::Router<AppState> {
    // Merge user-supplied raw Axum routers (escape hatch).
    // Merged after annotated routes so annotated routes take precedence.
    for raw_router in merge_routers {
        tracing::debug!("Merged raw Axum router");
        let raw_router = if let Some(layers) = idempotency_layers {
            raw_router.layer(layers.manual.clone())
        } else {
            raw_router
        };
        router = router.merge(raw_router);
    }

    // Nest user-supplied raw Axum routers under path prefixes.
    for (prefix, raw_router) in nest_routers {
        tracing::debug!(prefix = %prefix, "Nested raw Axum router");
        // We explicitly apply the fallback to the nested router before nesting,
        // so that unmatched routes within this prefix are protected by global middleware.
        let nested_router =
            raw_router.fallback(crate::middleware::error_page_filter::fallback_404_handler);
        let nested_router = if let Some(layers) = idempotency_layers {
            nested_router.layer(layers.manual.clone())
        } else {
            nested_router
        };
        router = router.nest(&prefix, nested_router);
    }
    router
}

fn apply_cors_middleware<S>(mut router: axum::Router<S>, config: &AutumnConfig) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // CORS middleware (only applied when allowed_origins is non-empty)
    if !config.cors.allowed_origins.is_empty() {
        let cors = build_cors_layer(&config.cors);
        tracing::info!(
            origins = ?config.cors.allowed_origins,
            credentials = config.cors.allow_credentials,
            "CORS enabled"
        );
        router = router.layer(cors);
    }
    router
}

fn apply_csrf_middleware<S>(
    mut router: axum::Router<S>,
    config: &AutumnConfig,
    signing_keys: Option<std::sync::Arc<crate::security::config::ResolvedSigningKeys>>,
) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // CSRF middleware (only applied when enabled)
    if config.security.csrf.enabled {
        let mut csrf_layer = crate::security::CsrfLayer::from_config(&config.security.csrf);
        if let Some(keys) = signing_keys {
            csrf_layer = csrf_layer.with_signing_keys(keys);
        }
        tracing::info!("CSRF protection enabled");
        router = router.layer(csrf_layer);
    }
    router
}

fn apply_rate_limit_middleware<S>(
    mut router: axum::Router<S>,
    config: &AutumnConfig,
) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // Rate limiting middleware (only applied when enabled)
    if config.security.rate_limit.enabled {
        let layer = crate::security::RateLimitLayer::from_config(&config.security.rate_limit);
        tracing::info!(
            rps = config.security.rate_limit.requests_per_second,
            burst = config.security.rate_limit.burst,
            "Rate limiting enabled"
        );
        router = router.layer(layer);
    }
    router
}

fn apply_upload_middleware<S>(router: axum::Router<S>, config: &AutumnConfig) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let upload_config = config.security.upload.clone();
    tracing::info!(
        max_request_size_bytes = upload_config.max_request_size_bytes,
        max_file_size_bytes = upload_config.max_file_size_bytes,
        allowed_mime_types = ?upload_config.allowed_mime_types,
        "Multipart upload safeguards enabled"
    );

    router.layer(axum::middleware::from_fn(
        move |mut req: axum::extract::Request, next: axum::middleware::Next| {
            let upload_config = upload_config.clone();
            async move {
                req.extensions_mut().insert(upload_config);
                next.run(req).await
            }
        },
    ))
}

struct BuiltIdempotencyLayers {
    route: crate::idempotency::IdempotencyLayer,
    manual: crate::idempotency::IdempotencyLayer,
}

fn build_idempotency_layers(
    config: &AutumnConfig,
    state: &AppState,
) -> Result<Option<BuiltIdempotencyLayers>, RouterBuildError> {
    if !config.idempotency.enabled.unwrap_or(false) {
        return Ok(None);
    }

    let ttl = Duration::from_secs(config.idempotency.ttl_secs);
    let in_flight_ttl = Duration::from_secs(config.idempotency.in_flight_ttl_secs);
    let store: std::sync::Arc<dyn IdempotencyStore> = match config.idempotency.backend {
        crate::config::IdempotencyBackend::Memory => {
            std::sync::Arc::new(MemoryIdempotencyStore::new(ttl))
        }
        #[cfg(feature = "redis")]
        crate::config::IdempotencyBackend::Redis => {
            match crate::idempotency::RedisIdempotencyStore::from_config(&config.idempotency) {
                Ok(s) => std::sync::Arc::new(s),
                Err(e) => return Err(RouterBuildError::InvalidIdempotencyBackend(e)),
            }
        }
        #[cfg(not(feature = "redis"))]
        crate::config::IdempotencyBackend::Redis => {
            return Err(RouterBuildError::InvalidIdempotencyBackend(
                "idempotency backend 'redis' requires the autumn-web 'redis' feature \
                 flag; rebuild with --features redis or switch to backend = \"memory\""
                    .to_owned(),
            ));
        }
    };

    tracing::debug!(
        backend = ?config.idempotency.backend,
        ttl_secs = config.idempotency.ttl_secs,
        in_flight_ttl_secs = config.idempotency.in_flight_ttl_secs,
        "Idempotency-key middleware enabled"
    );

    let base = IdempotencyLayer::new(store)
        .with_ttl(ttl)
        .with_in_flight_ttl(in_flight_ttl)
        .with_metrics(state.metrics.clone());

    Ok(Some(BuiltIdempotencyLayers {
        route: base.clone().replay_through_inner(),
        manual: base.fail_closed_on_replay(),
    }))
}

#[allow(clippy::cognitive_complexity)]
fn apply_middleware(
    mut router: axum::Router<AppState>,
    config: &AutumnConfig,
    state: &AppState,
    exception_filters: Vec<Arc<dyn ExceptionFilter>>,
    custom_layers: Vec<crate::app::CustomLayerRegistration>,
    error_page_renderer: Option<SharedRenderer>,
    session_store: Option<Arc<dyn crate::session::BoxedSessionStore>>,
) -> Result<axum::Router<AppState>, RouterBuildError> {
    // 404 fallback handler for unmatched routes must be registered BEFORE global middleware
    // so that unmatched routes are still protected by rate limiting, CSRF, CORS, etc.
    router = router.fallback(crate::middleware::error_page_filter::fallback_404_handler);

    // Resolve signing keys once; shared across session and CSRF layers.
    let is_production = matches!(config.profile.as_deref(), Some("prod" | "production"));
    let signing_keys = std::sync::Arc::new(crate::security::config::resolve_signing_keys(
        &config.security.signing_secret,
    ));
    // Only thread signing keys when a secret is configured (or in production where
    // fail_fast already ensures one is present). In dev without a configured secret
    // the ephemeral key is generated per-process — useful but not required.
    let signing_keys_opt: Option<std::sync::Arc<crate::security::config::ResolvedSigningKeys>> =
        if config.security.signing_secret.secret.is_some() || is_production {
            Some(signing_keys)
        } else {
            None
        };

    router = apply_cors_middleware(router, config);
    let trusted_host_policy = TrustedHostPolicy::from_config(config);
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        trusted_host_middleware(req, next, trusted_host_policy.clone())
    }));
    router = apply_csrf_middleware(router, config, signing_keys_opt.clone());
    // Method-override rejection filter. The outer `MethodOverrideLayer`
    // (applied at the `axum::serve` boundary so it can rewrite the
    // request method before route matching) stamps a
    // [`MethodOverrideRejection`] extension when the override field
    // value is invalid or the body was too large to scan; this inner
    // middleware converts that extension into the corresponding
    // `400`/`413` response. Running it here means the rejection flows
    // through the rest of the response stack (security headers,
    // request IDs, metrics, error-page filter) rather than bypassing
    // them. Placed outside CSRF so a `BodyTooLarge` (empty body)
    // doesn't get masked by a `403` from CSRF's missing-token branch,
    // and a clear `400 invalid _method` outranks "missing CSRF".
    router = router.layer(axum::middleware::from_fn(
        crate::middleware::method_override_rejection_filter,
    ));
    router = apply_rate_limit_middleware(router, config);
    router = apply_upload_middleware(router, config);

    // Security headers layer (always applied)
    let security_headers =
        crate::security::SecurityHeadersLayer::from_config(&config.security.headers);
    tracing::debug!("Security headers enabled");

    // User-registered Tower layers (AppBuilder::layer). Outermost — applied
    // last so they wrap all framework middleware.  Iterate in reverse so the
    // first registered layer ends up outermost among user layers — matching
    // tower::ServiceBuilder ordering.
    //
    // When a static dist dir is active (SSG/ISG build), these layers are
    // NOT passed here — they are extracted by try_build_router_with_static_inner
    // and applied outside the static-first middleware instead, so they can
    // process pre-rendered responses without creating a session dependency.
    let custom_layer_count = custom_layers.len();
    for registered in custom_layers.into_iter().rev() {
        router = (registered.apply)(router);
    }
    if custom_layer_count > 0 {
        tracing::debug!(count = custom_layer_count, "Custom Tower layers applied");
    }

    let mut router = router;

    if config.tenancy.enabled {
        router = router.layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::tenancy::tenancy_middleware,
        ));
        tracing::debug!("Multi-tenancy middleware enabled");
    }

    let router = router.layer(RequestIdLayer).layer(security_headers);

    let router = crate::session::apply_session_layer(
        router,
        &config.session,
        config.profile.as_deref(),
        session_store,
        signing_keys_opt,
    )?;
    tracing::debug!(backend = ?config.session.backend, "Session management enabled");

    // Error page filter: renders HTML error pages for browser requests.
    // Always registered (uses default renderer if no custom one is provided).
    let is_dev = config
        .profile
        .as_deref()
        .map_or(cfg!(debug_assertions), |p| p == "dev");
    let renderer = error_page_renderer.unwrap_or_else(error_pages::default_renderer);
    let error_page_filter = crate::middleware::error_page_filter::ErrorPageFilter {
        renderer,
        is_dev,
        parameter_filter: crate::log::filter::ParameterFilter::new(
            &config.log.filter_parameters,
            &config.log.unfilter_parameters,
        ),
    };

    // Combine the Problem Details normalizer and error page filter with user
    // exception filters. Problem Details runs first so HTML negotiation can
    // still replace the JSON response for browser requests.
    let mut all_filters: Vec<Arc<dyn ExceptionFilter>> = vec![
        Arc::new(ProblemDetailsFilter { is_dev }),
        Arc::new(error_page_filter),
    ];
    all_filters.extend(exception_filters);

    let count = all_filters.len();
    tracing::debug!(
        count,
        "Registered exception filters (including error page filter)"
    );

    // Error page context layer must be inner to the exception filter so
    // WantsHtml is set on the response before the filter inspects it.
    // Full ingress layer order (outermost -> innermost):
    //   TraceContext (applied outside the startup barrier so short-circuit
    //   responses still carry traceparent) ->
    //   [user layers, when SSG/ISG dist dir active] ->
    //   StaticFileMiddleware (when SSG/ISG enabled) ->
    //   Metrics -> ExceptionFilter -> ErrorPageContext -> Session ->
    //   SecurityHeaders -> RequestId -> [user layers, non-static build] ->
    //   RateLimit -> CSRF -> CORS -> handler
    let router = router
        .layer(crate::middleware::error_page_filter::ErrorPageContextLayer)
        .layer(ExceptionFilterLayer::new(all_filters))
        .layer(crate::middleware::MetricsLayer::new(state.metrics.clone()));

    Ok(router)
}

async fn trusted_host_middleware(
    req: Request<axum::body::Body>,
    next: Next,
    policy: TrustedHostPolicy,
) -> axum::response::Response {
    let path = req.uri().path();
    if (req.method() == http::Method::GET || req.method() == http::Method::HEAD)
        && policy.probe_bypass_paths.contains(path)
    {
        return next.run(req).await;
    }
    let authority = req.uri().authority().map(http::uri::Authority::as_str);
    let host_header = req
        .headers()
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok());
    let raw_host = authority.or(host_header);
    let parsed_host = raw_host.and_then(extract_host_without_port);
    let host = parsed_host
        .map(str::to_ascii_lowercase)
        .map(|h| h.trim_end_matches('.').to_owned())
        .filter(|h| !h.is_empty());
    let host_source_present = raw_host.is_some();
    if host.is_none() && !host_source_present && policy.allow_missing_host {
        return next.run(req).await;
    }
    if host.as_deref().is_some_and(|host| policy.allows_host(host)) {
        next.run(req).await
    } else {
        tracing::warn!(host = ?host, "trusted host rejected request");
        let body = crate::error::problem_details_json_string(
            StatusCode::BAD_REQUEST,
            "Invalid Host header",
            None,
            None,
            None,
            None,
            true,
        );
        (
            StatusCode::BAD_REQUEST,
            [(http::header::CONTENT_TYPE, "application/problem+json")],
            body,
        )
            .into_response()
    }
}

fn extract_host_without_port(header: &str) -> Option<&str> {
    let host = header.trim();
    if host.is_empty() {
        return None;
    }
    if host.starts_with('[') {
        let end = host.find(']')?;
        let literal = host.get(1..end)?;
        if literal.is_empty() || literal.parse::<std::net::IpAddr>().is_err() {
            return None;
        }

        let remainder = host.get(end + 1..)?;
        if remainder.is_empty() {
            return Some(literal);
        }

        let maybe_port = remainder.strip_prefix(':')?;
        if !maybe_port.is_empty() && maybe_port.chars().all(|c| c.is_ascii_digit()) {
            return Some(literal);
        }

        return None;
    }
    let Some((candidate, maybe_port)) = host.rsplit_once(':') else {
        return Some(host);
    };
    if candidate.contains(':') {
        // unbracketed IPv6 literal; keep host verbatim
        return Some(host);
    }
    if !maybe_port.is_empty()
        && maybe_port.chars().all(|c| c.is_ascii_digit())
        && !candidate.is_empty()
    {
        Some(candidate)
    } else {
        None
    }
}

/// Build the router with optional static-file-first serving.
///
/// If `dist_dir` is `Some` and contains a valid `manifest.json`, the
/// returned router intercepts GET/HEAD requests whose path appears in
/// the manifest and serves pre-built HTML directly — before the dynamic
/// router runs.  This matches Next.js SSG/ISR semantics where static
/// pages always win over dynamic handlers.
///
/// Requests not in the manifest (including non-GET/HEAD methods) fall
/// through to the dynamic router unchanged.
///
/// When `dist_dir` is `None` or the manifest is missing, the returned
/// router is identical to [`build_router`].
///
/// This function is public primarily for integration testing.
///
/// # Panics
///
/// Panics when framework router assembly encounters invalid configuration.
/// Use [`try_build_router_with_static`] to handle configuration errors
/// explicitly.
#[allow(dead_code)]
pub fn build_router_with_static(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    dist_dir: Option<&std::path::Path>,
) -> axum::Router {
    try_build_router_with_static(route_list, config, state, dist_dir)
        .unwrap_or_else(|error| panic!("invalid router configuration: {error}"))
}

/// Checked variant of [`build_router_with_static`] that returns configuration
/// errors instead of panicking.
///
/// # Errors
///
/// Returns [`RouterBuildError`] when router assembly encounters invalid
/// framework configuration, such as an unusable session backend.
#[allow(dead_code)]
pub fn try_build_router_with_static(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    dist_dir: Option<&std::path::Path>,
) -> Result<axum::Router, RouterBuildError> {
    try_build_router_with_static_inner(
        route_list,
        config,
        state,
        dist_dir,
        RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            #[cfg(feature = "openapi")]
            openapi: None,
        },
    )
}

pub fn try_build_router_with_static_inner(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    dist_dir: Option<&std::path::Path>,
    mut ctx: RouterContext,
) -> Result<axum::Router, RouterBuildError> {
    let startup_barrier_state = state.clone();

    let Some(dist) = dist_dir else {
        let app_router = try_build_router_inner(route_list, config, state, ctx)?;
        return Ok(apply_startup_barrier(
            app_router,
            config,
            &startup_barrier_state,
        ));
    };

    let Some(layer) = crate::static_gen::StaticFileLayer::new(dist) else {
        tracing::debug!(
            dist = %dist.display(),
            "No valid manifest.json in dist dir; skipping static file layer"
        );
        let app_router = try_build_router_inner(route_list, config, state, ctx)?;
        return Ok(apply_startup_barrier(
            app_router,
            config,
            &startup_barrier_state,
        ));
    };

    for (route, entry) in &layer.manifest().routes {
        tracing::debug!(
            route = %route,
            file = %entry.file,
            revalidate = ?entry.revalidate,
            "Static route"
        );
    }

    // Extract user layers before building the inner router. They are applied
    // OUTSIDE the static-first middleware (and outside session) so that:
    //   • User layers (e.g. compression) can process pre-rendered responses.
    //   • Static serving remains available even if the session backend is down.
    //   • ISR regeneration uses the inner router (no user layers), ensuring
    //     re-rendered pages are saved as raw HTML rather than pre-transformed.
    //
    // Compute the idempotency flag NOW while custom_layers is still populated,
    // then drain it. build_router_pre_state would otherwise see an empty list
    // and incorrectly treat opaque layers as absent when selecting idempotency
    // behaviour for each route.
    let opaque_present = Some(custom_layers_require_fail_closed_idempotency(
        &ctx.custom_layers,
    ));
    let custom_layers = std::mem::take(&mut ctx.custom_layers);

    let inner_router = build_router_pre_state(route_list, config, &state, ctx, opaque_present)?;

    // Attach the inner router for ISR background regeneration. Because user
    // layers are excluded, re-renders produce raw HTML (no compression, etc.)
    // that is then saved to disk and served with user-layer processing applied
    // at request time.
    let has_isr = layer
        .manifest()
        .routes
        .values()
        .any(|e| e.revalidate.is_some());
    let layer = if has_isr {
        layer.with_router(inner_router.clone().with_state(state.clone()))
    } else {
        layer
    };
    let layer = Arc::new(layer);

    // Static-first serving: intercept GET/HEAD requests whose path appears
    // in the manifest and serve pre-built HTML directly — BEFORE the dynamic
    // router (and session layer) runs. This preserves availability of static
    // pages even when the session backend is unavailable.
    //
    // Requests not in the manifest (including non-GET/HEAD methods) fall
    // through to the dynamic router unchanged.
    //
    // ISR staleness checking happens inside `resolve()`: stale pages are
    // still served immediately while background regeneration runs
    // (stale-while-revalidate).
    let static_layer = layer;
    let mut router: axum::Router<AppState> = inner_router.layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let static_layer = static_layer.clone();
            async move {
                let is_get = req.method() == http::Method::GET;
                let is_head = req.method() == http::Method::HEAD;
                if is_get || is_head {
                    let path = req.uri().path();
                    // Normalize trailing slash: /about/ → /about (but keep / as /)
                    let normalized = if path.len() > 1 && path.ends_with('/') {
                        &path[..path.len() - 1]
                    } else {
                        path
                    };
                    if let Some(file_path) = static_layer.resolve(normalized)
                        && let Ok(contents) = tokio::fs::read(&file_path).await
                    {
                        let body = if is_head {
                            axum::body::Body::empty()
                        } else {
                            axum::body::Body::from(contents)
                        };
                        return http::Response::builder()
                            .status(http::StatusCode::OK)
                            .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                            .body(body)
                            .expect("infallible response builder");
                    }
                }
                next.run(req).await
            }
        },
    ));

    // Apply user layers OUTSIDE the static middleware so they wrap it and can
    // process both static and dynamic responses (e.g. compress the HTML on
    // the way out). Iterate in reverse so the first registered layer ends up
    // outermost — matching tower::ServiceBuilder ordering.
    let custom_layer_count = custom_layers.len();
    for registered in custom_layers.into_iter().rev() {
        router = (registered.apply)(router);
    }
    if custom_layer_count > 0 {
        tracing::debug!(
            count = custom_layer_count,
            "Custom Tower layers applied outside static middleware"
        );
    }

    let router = router.layer(crate::security::SecurityHeadersLayer::from_config(
        &config.security.headers,
    ));

    Ok(apply_startup_barrier(
        router.with_state(state),
        config,
        &startup_barrier_state,
    ))
}

#[derive(Clone)]
struct StartupBarrierState {
    app_state: AppState,
    live_path: String,
    ready_path: String,
    startup_path: String,
    health_path: String,
    actuator_paths: Vec<String>,
    actuator_subtree_paths: Vec<String>,
}

impl StartupBarrierState {
    fn from_config(config: &AutumnConfig, app_state: &AppState) -> Self {
        let actuator_subtree_paths = if config.actuator.sensitive {
            vec![crate::actuator::actuator_route_path(
                &config.actuator.prefix,
                "/loggers",
            )]
        } else {
            Vec::new()
        };

        Self {
            app_state: app_state.clone(),
            live_path: config.health.live_path.clone(),
            ready_path: config.health.ready_path.clone(),
            startup_path: config.health.startup_path.clone(),
            health_path: config.health.path.clone(),
            actuator_paths: crate::actuator::actuator_endpoint_paths(
                &config.actuator.prefix,
                config.actuator.sensitive,
            ),
            actuator_subtree_paths,
        }
    }

    fn allows_path(&self, path: &str) -> bool {
        path == self.live_path
            || path == self.ready_path
            || path == self.startup_path
            || path == self.health_path
            || self.actuator_paths.iter().any(|allowed| path == allowed)
            || self
                .actuator_subtree_paths
                .iter()
                .any(|allowed| path_matches_route_prefix(path, allowed))
    }
}

fn apply_startup_barrier(
    router: axum::Router,
    config: &AutumnConfig,
    state: &AppState,
) -> axum::Router {
    let barrier_state = StartupBarrierState::from_config(config, state);
    let router = router.layer(axum::middleware::from_fn_with_state(
        barrier_state,
        startup_barrier,
    ));
    // W3C Trace Context propagation wraps the startup barrier (and the
    // static-first middleware above it) so short-circuit responses —
    // startup 503s and pre-built static file hits — still extract the
    // incoming `traceparent` and inject the current context into the
    // outgoing response. Applied here rather than inside `apply_middleware`
    // because those outer wrappers can return without ever invoking the
    // inner router.
    #[cfg(feature = "telemetry-otlp")]
    let router = router.layer(crate::middleware::TraceContextLayer);
    router
}

async fn startup_barrier(
    State(state): State<StartupBarrierState>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    if crate::app::is_static_build_mode()
        || state.app_state.probes().is_startup_complete()
        || state.allows_path(request.uri().path())
    {
        next.run(request).await
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Service is still starting up",
        )
            .into_response()
    }
}

fn path_matches_route_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Build a `tower_http::cors::CorsLayer` from the framework's [`crate::config::CorsConfig`].
///
/// Called only when `config.cors.allowed_origins` is non-empty.
pub fn build_cors_layer(cors: &crate::config::CorsConfig) -> tower_http::cors::CorsLayer {
    use http::header::HeaderName;
    use tower_http::cors::{AllowOrigin, CorsLayer};

    let layer = if cors.allowed_origins.iter().any(|o| o == "*") {
        CorsLayer::new().allow_origin(AllowOrigin::any())
    } else {
        let origins: Vec<http::HeaderValue> = cors
            .allowed_origins
            .iter()
            .filter_map(|o| match o.parse() {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(origin = %o, error = %e, "CORS: ignoring malformed allowed_origin");
                    None
                }
            })
            .collect();
        CorsLayer::new().allow_origin(origins)
    };

    let methods: Vec<http::Method> = cors
        .allowed_methods
        .iter()
        .filter_map(|m| match m.parse() {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(method = %m, error = %e, "CORS: ignoring malformed allowed_method");
                None
            }
        })
        .collect();

    let headers: Vec<HeaderName> = cors
        .allowed_headers
        .iter()
        .filter_map(|h| match h.parse() {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(header = %h, error = %e, "CORS: ignoring malformed allowed_header");
                None
            }
        })
        .collect();

    layer
        .allow_methods(methods)
        .allow_headers(headers)
        .allow_credentials(cors.allow_credentials)
        .max_age(std::time::Duration::from_secs(cors.max_age_secs))
}

/// Set `Cache-Control` headers for static assets based on whether the path is
/// fingerprinted.
///
/// | Path | Header |
/// |------|--------|
/// | `/static/**.<8hex>.*` | `public, max-age=31536000, immutable` |
/// | `/static/**` (other) | `public, max-age=0, must-revalidate` |
/// | Everything else | unchanged |
///
/// The short `must-revalidate` policy for plain static paths ensures that
/// returning visitors always fetch the latest file after a deploy, while the
/// long `immutable` policy for fingerprinted files lets browsers skip the
/// network entirely for assets whose content will never change.
pub async fn asset_cache_control(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path().to_owned();
    let mut resp = next.run(req).await;
    if path.starts_with("/static/") && resp.status().is_success() {
        // Use manifest membership rather than filename pattern so that
        // user-authored assets like `vendor.deadbeef.js` are never given an
        // immutable cache lifetime.
        let is_immutable = path
            .strip_prefix("/static/")
            .is_some_and(crate::assets::is_manifest_asset);
        let header = if is_immutable {
            "public, max-age=31536000, immutable"
        } else {
            "public, max-age=0, must-revalidate"
        };
        resp.headers_mut().insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static(header),
        );
    }
    resp
}

#[cfg(feature = "htmx")]
pub async fn htmx_handler() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [
            (http::header::CONTENT_TYPE, "application/javascript"),
            (
                http::header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ),
        ],
        crate::htmx::HTMX_JS,
    )
        .into_response()
}

#[cfg(feature = "htmx")]
pub async fn htmx_csrf_handler() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [
            (http::header::CONTENT_TYPE, "application/javascript"),
            (
                http::header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ),
        ],
        crate::htmx::HTMX_CSRF_JS,
    )
        .into_response()
}

#[cfg(feature = "openapi")]
fn collect_openapi_docs(
    route_list: &[Route],
    scoped_groups: &[ScopedGroup],
) -> Vec<crate::openapi::ApiDoc> {
    // Walk both top-level routes and scoped groups. For scoped groups the
    // effective path is `prefix + route.path`; we materialize these into
    // fresh `ApiDoc`s so the rendered spec reflects the actual URL the
    // user will call.
    let mut docs: Vec<crate::openapi::ApiDoc> = Vec::new();
    for route in route_list {
        docs.push(route.api_doc.clone());
    }
    for group in scoped_groups {
        // Extract `{name}` captures from the scope prefix so parameters
        // declared in the prefix (e.g. `/orgs/{org_id}`) show up on the
        // generated operation alongside the child route's own params.
        let prefix_params = extract_path_params(&group.prefix);
        for route in &group.routes {
            let mut doc = route.api_doc.clone();
            // Leak the combined path so it fits the `&'static str` shape of
            // ApiDoc. The spec is built once per process; the leak is
            // bounded by the route table size. Using the same
            // normalization as `join_nested_path` keeps the spec's
            // paths aligned with the URLs axum actually routes.
            let full = join_nested_path(&group.prefix, route.api_doc.path);
            doc.path = Box::leak(full.into_boxed_str());

            if !prefix_params.is_empty() {
                let mut merged: Vec<&'static str> = prefix_params
                    .iter()
                    .map(|p| &*Box::leak(p.clone().into_boxed_str()))
                    .collect();
                for existing in route.api_doc.path_params {
                    if !merged.iter().any(|n| n == existing) {
                        merged.push(existing);
                    }
                }
                doc.path_params = Box::leak(merged.into_boxed_slice());
            }

            docs.push(doc);
        }
    }
    docs
}

#[cfg(feature = "openapi")]
fn mount_swagger_ui_routes(
    mut router: axum::Router<AppState>,
    path: &str,
    title: &str,
    json_path: &str,
) -> axum::Router<AppState> {
    let [css_path, bundle_path, initializer_path] = crate::openapi::swagger_ui_asset_paths(path);
    let html_body = Arc::new(crate::openapi::swagger_ui_html(
        title,
        &css_path,
        &bundle_path,
        &initializer_path,
    ));
    let initializer_body = Arc::new(crate::openapi::swagger_ui_initializer_js(json_path));
    router = router.route(
        path,
        axum::routing::get(move || {
            let html = html_body.clone();
            async move {
                use axum::response::IntoResponse;
                (
                    [(http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    (*html).clone(),
                )
                    .into_response()
            }
        }),
    );
    router = router.route(
        &css_path,
        axum::routing::get(|| async move {
            use axum::response::IntoResponse;
            (
                [(http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
                crate::openapi::SWAGGER_UI_CSS,
            )
                .into_response()
        }),
    );
    router = router.route(
        &bundle_path,
        axum::routing::get(|| async move {
            use axum::body::Bytes;
            use axum::response::IntoResponse;
            (
                [(
                    http::header::CONTENT_TYPE,
                    "application/javascript; charset=utf-8",
                )],
                Bytes::from_static(crate::openapi::SWAGGER_UI_BUNDLE),
            )
                .into_response()
        }),
    );
    router = router.route(
        &initializer_path,
        axum::routing::get(move || {
            let js = initializer_body.clone();
            async move {
                use axum::response::IntoResponse;
                (
                    [(
                        http::header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    (*js).clone(),
                )
                    .into_response()
            }
        }),
    );
    router
}

#[cfg(feature = "oauth2")]
async fn http_interceptor_middleware(
    state: axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use crate::interceptor::{ACTIVE_HTTP_INTERCEPTORS, HttpInterceptor};
    if let Some(interceptor_arc) = state.extension::<Arc<dyn HttpInterceptor>>() {
        let interceptor = (*interceptor_arc).clone();
        let interceptors = vec![interceptor];
        ACTIVE_HTTP_INTERCEPTORS
            .scope(interceptors, async move { next.run(req).await })
            .await
    } else {
        next.run(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            #[cfg(feature = "db")]
            replica_pool: None,
            profile: Some("test".to_owned()),
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
        }
    }

    #[tokio::test]
    async fn build_router_mounts_actuator_at_configured_prefix() {
        let mut config = AutumnConfig::default();
        config.actuator.prefix = "/ops".to_owned();
        config.actuator.sensitive = true;

        let app = build_router(Vec::new(), &config, test_state());

        let prefixed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ops/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(prefixed.status(), StatusCode::OK);

        let legacy = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn try_build_router_rejects_invalid_session_backend_config() {
        let mut config = AutumnConfig::default();
        config.session.backend = crate::session::SessionBackend::Redis;

        let error = try_build_router(Vec::new(), &config, test_state())
            .expect_err("missing redis config should fail checked router build");

        assert!(matches!(
            error,
            RouterBuildError::InvalidSessionBackend(
                crate::session::SessionBackendConfigError::MissingRedisUrl
            )
        ));
    }

    #[test]
    fn try_build_router_with_static_rejects_invalid_session_backend_config() {
        let mut config = AutumnConfig::default();
        config.session.backend = crate::session::SessionBackend::Redis;

        let error = try_build_router_with_static(Vec::new(), &config, test_state(), None)
            .expect_err("missing redis config should fail checked static router build");

        assert!(matches!(
            error,
            RouterBuildError::InvalidSessionBackend(
                crate::session::SessionBackendConfigError::MissingRedisUrl
            )
        ));
    }

    #[test]
    fn try_build_router_returns_error_for_probe_actuator_path_overlap() {
        let mut config = AutumnConfig::default();
        config.actuator.prefix = "/".to_owned();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            try_build_router(Vec::new(), &config, test_state())
        }));

        assert!(result.is_ok(), "try_build_router panicked on route overlap");
        assert!(
            result.unwrap().is_err(),
            "route overlap should be reported as a checked router build error"
        );
    }

    #[tokio::test]
    async fn apply_cors_middleware_skipped_when_no_origins() {
        let config = AutumnConfig::default();
        assert!(config.cors.allowed_origins.is_empty());

        let base: axum::Router<AppState> =
            axum::Router::new().route("/test", axum::routing::get(|| async { "ok" }));
        let router = apply_cors_middleware(base, &config).with_state(test_state());

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("Origin", "https://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "CORS header must be absent when no origins are configured"
        );
    }

    #[tokio::test]
    async fn apply_cors_middleware_present_when_origins_configured() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["https://example.com".to_owned()];

        let base: axum::Router<AppState> =
            axum::Router::new().route("/test", axum::routing::get(|| async { "ok" }));
        let router = apply_cors_middleware(base, &config).with_state(test_state());

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("Origin", "https://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_some(),
            "CORS header must be present when origins are configured"
        );
    }

    #[tokio::test]
    async fn apply_cors_middleware_handles_preflight_request() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["https://example.com".to_owned()];

        let base: axum::Router<AppState> =
            axum::Router::new().route("/api/widgets", axum::routing::post(|| async { "ok" }));
        let router = apply_cors_middleware(base, &config).with_state(test_state());

        let response = router
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/widgets")
                    .header("Origin", "https://example.com")
                    .header("Access-Control-Request-Method", "POST")
                    .header("Access-Control-Request-Headers", "Content-Type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let headers = response.headers();
        assert_eq!(
            headers
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("https://example.com"),
            "preflight must echo the allowed origin"
        );
        assert!(
            headers.get("access-control-allow-methods").is_some(),
            "preflight must advertise allowed methods"
        );
        assert!(
            headers.get("access-control-allow-headers").is_some(),
            "preflight must advertise allowed headers"
        );
        assert!(
            headers.get("access-control-max-age").is_some(),
            "preflight must advertise max-age so browsers can cache it"
        );
    }

    #[tokio::test]
    async fn apply_csrf_middleware_skipped_when_disabled() {
        let config = AutumnConfig::default();
        assert!(!config.security.csrf.enabled);

        let base: axum::Router<AppState> =
            axum::Router::new().route("/form", axum::routing::post(|| async { "posted" }));
        let router = apply_csrf_middleware(base, &config, None).with_state(test_state());

        // Without CSRF the POST should pass through with no CSRF-specific response
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn apply_rate_limit_middleware_skipped_when_disabled() {
        let config = AutumnConfig::default();
        assert!(!config.security.rate_limit.enabled);

        let base: axum::Router<AppState> =
            axum::Router::new().route("/ping", axum::routing::get(|| async { "pong" }));
        let router = apply_rate_limit_middleware(base, &config).with_state(test_state());

        // Fire several rapid requests; none should be throttled.
        for _ in 0..5 {
            let response = router
                .clone()
                .oneshot(Request::builder().uri("/ping").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn apply_rate_limit_middleware_returns_429_when_exhausted() {
        let mut config = AutumnConfig::default();
        config.security.rate_limit.enabled = true;
        config.security.rate_limit.requests_per_second = 0.1;
        config.security.rate_limit.burst = 1;
        config.security.rate_limit.trust_forwarded_headers = true;

        let base: axum::Router<AppState> =
            axum::Router::new().route("/ping", axum::routing::get(|| async { "pong" }));
        let router = apply_rate_limit_middleware(base, &config).with_state(test_state());

        let ok = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ping")
                    .header("X-Forwarded-For", "203.0.113.9")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);

        let blocked = router
            .oneshot(
                Request::builder()
                    .uri("/ping")
                    .header("X-Forwarded-For", "203.0.113.9")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(blocked.headers().get("retry-after").is_some());
    }

    #[cfg(feature = "mail")]
    fn dev_mail_preview_config(dir: &std::path::Path) -> AutumnConfig {
        let mut config = AutumnConfig {
            profile: Some("dev".to_owned()),
            mail: crate::mail::MailConfig {
                transport: crate::mail::Transport::File,
                file_dir: dir.to_path_buf(),
                ..Default::default()
            },
            ..Default::default()
        };
        config.security.trusted_hosts.hosts = vec!["example.com".to_owned()];
        config
    }

    #[cfg(feature = "mail")]
    async fn response_text(response: axum::response::Response) -> String {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should collect");
        String::from_utf8(body.to_vec()).expect("body should be utf8")
    }

    #[cfg(feature = "mail")]
    #[tokio::test]
    async fn build_router_mounts_dev_mail_preview_empty_state_for_file_transport() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dev_mail_preview_config(dir.path());
        let router = build_router(Vec::new(), &config, test_state());

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/_autumn/mail")
                    .header("host", "example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        assert!(
            body.contains("No captured emails"),
            "missing empty state: {body}"
        );
        assert!(
            body.contains("mail.transport = &quot;file&quot;"),
            "empty state should explain capture setup: {body}"
        );
    }

    #[cfg(feature = "mail")]
    #[tokio::test]
    async fn build_router_lists_captured_mail_newest_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let older = dir.path().join("older.eml");
        let newer = dir.path().join("newer.eml");
        std::fs::write(
            &older,
            "To: first@example.com\nSubject: First\nDate: Tue, 05 May 2026 10:00:00 +0000\nMessage-Id: <first@example.com>\n\nfirst body\n",
        )
        .expect("write older eml");
        std::fs::write(
            &newer,
            "To: second@example.com\nSubject: Second\nDate: Tue, 05 May 2026 10:01:00 +0000\nMessage-Id: <second@example.com>\n\nsecond body\n",
        )
        .expect("write newer eml");
        filetime::set_file_mtime(&older, filetime::FileTime::from_unix_time(100, 0))
            .expect("set older mtime");
        filetime::set_file_mtime(&newer, filetime::FileTime::from_unix_time(200, 0))
            .expect("set newer mtime");

        let config = dev_mail_preview_config(dir.path());
        let router = build_router(Vec::new(), &config, test_state());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/_autumn/mail")
                    .header("host", "example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        let second = body.find("Second").expect("newer subject should render");
        let first = body.find("First").expect("older subject should render");
        assert!(second < first, "newest message should render first: {body}");
        assert!(
            body.contains("second@example.com"),
            "missing To column: {body}"
        );
        assert!(
            body.contains("Timestamp"),
            "missing timestamp column: {body}"
        );
    }

    #[cfg(feature = "mail")]
    #[tokio::test]
    async fn build_router_mail_preview_detail_renders_html_in_sandboxed_iframe() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("detail.eml"),
            "From: Autumn <noreply@example.com>\nTo: ada@example.com\nReply-To: support@example.com\nSubject: Reset\nDate: Tue, 05 May 2026 10:00:00 +0000\nMessage-Id: <reset@example.com>\nMIME-Version: 1.0\nContent-Type: multipart/alternative; boundary=\"autumn-mail\"\n\n--autumn-mail\nContent-Type: text/plain; charset=utf-8\n\nPlain reset\n--autumn-mail\nContent-Type: text/html; charset=utf-8\n\n<h1>Hello iframe</h1>\n--autumn-mail--\n",
        )
        .expect("write detail eml");

        let config = dev_mail_preview_config(dir.path());
        let router = build_router(Vec::new(), &config, test_state());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/_autumn/mail/messages/detail.eml")
                    .header("host", "example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        assert!(body.contains("<iframe"), "missing iframe: {body}");
        assert!(body.contains("sandbox"), "iframe must be sandboxed: {body}");
        assert!(body.contains("Hello iframe"), "missing html body: {body}");
        assert!(body.contains("Plain text"), "missing text toggle: {body}");
        assert!(body.contains("Headers"), "missing headers toggle: {body}");
        assert!(
            body.contains("Raw .eml"),
            "missing raw source toggle: {body}"
        );
        assert!(
            body.contains("Message-Id"),
            "missing message id header: {body}"
        );
    }

    #[cfg(feature = "mail")]
    #[tokio::test]
    async fn build_router_does_not_mount_mail_preview_outside_dev() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = dev_mail_preview_config(dir.path());
        config.profile = Some("prod".to_owned());
        let router = build_router(Vec::new(), &config, test_state());

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/_autumn/mail")
                    .header("host", "example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn apply_csrf_middleware_blocks_without_token_when_enabled() {
        let mut config = AutumnConfig::default();
        config.security.csrf.enabled = true;

        let base: axum::Router<AppState> =
            axum::Router::new().route("/form", axum::routing::post(|| async { "posted" }));
        let router = apply_csrf_middleware(base, &config, None).with_state(test_state());

        // POST without CSRF token should be rejected
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            response.status(),
            StatusCode::OK,
            "POST without CSRF token should be rejected when CSRF is enabled"
        );
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn join_nested_path_normalizes_like_axum() {
        // Reviewer's reported case: scope "/api" + child "/" must
        // produce "/api", not "/api/" — otherwise a user-configured
        // openapi_json_path("/api") won't match the effective mount
        // point and the collision check is unreliable.
        assert_eq!(super::join_nested_path("/api", "/"), "/api");
        // Trailing slash on prefix is stripped.
        assert_eq!(super::join_nested_path("/api/", "/"), "/api");
        // Normal case: prefix + child.
        assert_eq!(super::join_nested_path("/api", "/users"), "/api/users");
        // Trailing slash on prefix + child starting with slash doesn't
        // produce doubled slashes.
        assert_eq!(super::join_nested_path("/api/", "/users"), "/api/users");
        // Root prefix handles sensibly.
        assert_eq!(super::join_nested_path("", "/"), "/");
        assert_eq!(super::join_nested_path("", "/users"), "/users");
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn try_build_router_detects_scoped_root_collision() {
        // Scope "/api" + child "/" mounts axum's handler at "/api"
        // (not "/api/"). The collision check must use the same
        // normalization or we'd miss this overlap.
        use crate::openapi::{ApiDoc, OpenApiConfig};
        async fn child() -> &'static str {
            "inner"
        }
        let group = crate::route::ScopedGroup {
            prefix: "/api".to_owned(),
            routes: vec![Route {
                method: http::Method::GET,
                path: "/",
                handler: axum::routing::get(child),
                name: "root",
                api_doc: ApiDoc {
                    method: "GET",
                    path: "/",
                    operation_id: "root",
                    success_status: 200,
                    ..Default::default()
                },
                repository: None,
                idempotency: crate::route::RouteIdempotency::Direct,
            }],
            source: crate::route::RouteSource::User,
            apply_layer: Box::new(|r| r),
        };

        let openapi = OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("/api");
        let config = AutumnConfig::default();
        let ctx = RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: vec![group],
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            openapi: Some(openapi),
        };
        let err = super::try_build_router_inner(Vec::new(), &config, test_state(), ctx)
            .expect_err("scope '/api' + child '/' should collide with openapi path '/api'");
        assert!(matches!(
            err,
            RouterBuildError::OpenApiPathCollision {
                field: "openapi_json_path",
                ..
            }
        ));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn extract_path_params_matches_macro_behavior() {
        assert_eq!(
            super::extract_path_params("/orgs/{org_id}/users/{id}"),
            vec!["org_id".to_owned(), "id".to_owned()]
        );
        assert!(super::extract_path_params("/static").is_empty());
        assert_eq!(
            super::extract_path_params("/users/{id:[0-9]+}"),
            vec!["id".to_owned()]
        );
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn openapi_merges_scoped_prefix_path_params() {
        use crate::openapi::{ApiDoc, OpenApiConfig};

        // Scope prefix has `{org_id}`; the child route has `{id}`. The
        // generated ApiDoc must declare BOTH parameters, or Swagger
        // validators reject the document for referencing undeclared
        // path params.
        async fn handler() -> &'static str {
            "ok"
        }
        let child = Route {
            method: http::Method::GET,
            path: "/users/{id}",
            handler: axum::routing::get(handler),
            name: "child",
            api_doc: ApiDoc {
                method: "GET",
                path: "/users/{id}",
                operation_id: "child",
                path_params: &["id"],
                success_status: 200,
                ..Default::default()
            },
            repository: None,
            idempotency: crate::route::RouteIdempotency::Direct,
        };
        let group = crate::route::ScopedGroup {
            prefix: "/orgs/{org_id}".to_owned(),
            routes: vec![child],
            source: crate::route::RouteSource::User,
            apply_layer: Box::new(|r| r),
        };

        let config = OpenApiConfig::new("Demo", "1.0.0");
        let router = super::build_openapi_router(&[], &[group], Some(&config), "autumn.sid")
            .expect("openapi sub-router builds")
            .expect("openapi sub-router present when config is Some");
        let state = test_state();
        let router = router.with_state(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let params = &spec["paths"]["/orgs/{org_id}/users/{id}"]["get"]["parameters"];
        let names: Vec<&str> = params
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"org_id"), "missing org_id: {names:?}");
        assert!(names.contains(&"id"), "missing id: {names:?}");
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn openapi_documents_configured_session_cookie_name() {
        use crate::openapi::{ApiDoc, OpenApiConfig};

        async fn handler() -> &'static str {
            "ok"
        }

        let route = Route {
            method: http::Method::GET,
            path: "/protected",
            handler: axum::routing::get(handler),
            name: "protected",
            api_doc: ApiDoc {
                method: "GET",
                path: "/protected",
                operation_id: "protected",
                success_status: 200,
                secured: true,
                ..Default::default()
            },
            repository: None,
            idempotency: crate::route::RouteIdempotency::Direct,
        };

        let protected_routes = vec![route];
        let config = OpenApiConfig::new("Demo", "1.0.0");
        let docs_router =
            super::build_openapi_router(&protected_routes, &[], Some(&config), "demo.sid")
                .expect("openapi sub-router builds")
                .expect("openapi sub-router present when config is Some");
        let docs_router = docs_router.with_state(test_state());

        let response = docs_router
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let schemes = &spec["components"]["securitySchemes"];

        assert_eq!(schemes["SessionAuth"]["type"], "apiKey");
        assert_eq!(schemes["SessionAuth"]["in"], "cookie");
        assert_eq!(schemes["SessionAuth"]["name"], "demo.sid");
        assert!(
            schemes.get("BearerAuth").is_none(),
            "secured routes must not be documented as bearer JWT routes"
        );
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_json_path_without_leading_slash() {
        let config =
            crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("openapi.json");
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("non-slash path should be rejected");
        assert!(matches!(
            err,
            RouterBuildError::InvalidOpenApiPath {
                field: "openapi_json_path",
                ..
            }
        ));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_path_with_captures() {
        // `{id}` captures would be a typo for a mount path — the
        // endpoints are static. Catch it before axum panics.
        let config =
            crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("/docs/{id}");
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("captures should be rejected");
        assert!(matches!(err, RouterBuildError::InvalidOpenApiPath { .. }));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_path_with_unbalanced_brace() {
        let config =
            crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("/docs/{id");
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("unbalanced brace should be rejected");
        assert!(matches!(err, RouterBuildError::InvalidOpenApiPath { .. }));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_path_with_wildcard() {
        let config =
            crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("/docs/*rest");
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("wildcard should be rejected");
        assert!(matches!(err, RouterBuildError::InvalidOpenApiPath { .. }));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_path_with_double_slash() {
        let config =
            crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("//docs");
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("double-slash should be rejected");
        assert!(matches!(err, RouterBuildError::InvalidOpenApiPath { .. }));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_swagger_ui_path_without_leading_slash() {
        let config = crate::openapi::OpenApiConfig::new("Demo", "1.0.0")
            .swagger_ui_path(Some("docs".to_owned()));
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("non-slash path should be rejected");
        assert!(matches!(
            err,
            RouterBuildError::InvalidOpenApiPath {
                field: "swagger_ui_path",
                ..
            }
        ));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_empty_json_path() {
        let config = crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("");
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("empty path should be rejected");
        assert!(matches!(err, RouterBuildError::InvalidOpenApiPath { .. }));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_accepts_valid_paths() {
        let config = crate::openapi::OpenApiConfig::new("Demo", "1.0.0")
            .openapi_json_path("/api-docs")
            .swagger_ui_path(Some("/ui".to_owned()));
        let out = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect("valid paths must not error");
        assert!(out.is_some());
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn openapi_rejects_duplicate_json_and_swagger_paths() {
        let config = crate::openapi::OpenApiConfig::new("Demo", "1.0.0")
            .openapi_json_path("/docs")
            .swagger_ui_path(Some("/docs".to_owned()));
        let err = super::build_openapi_router(&[], &[], Some(&config), "autumn.sid")
            .expect_err("colliding paths should be rejected before axum panics");
        assert!(matches!(
            err,
            RouterBuildError::DuplicateOpenApiPath { ref path } if path == "/docs"
        ));
    }

    #[cfg(feature = "openapi")]
    async fn collision_test_handler() -> &'static str {
        "user"
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn try_build_router_rejects_openapi_path_colliding_with_user_route() {
        let mut config = AutumnConfig::default();
        config.actuator.prefix = "/ops".to_owned();
        let openapi =
            crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("/my-api-docs");

        let user_route = Route {
            method: http::Method::GET,
            path: "/my-api-docs",
            handler: axum::routing::get(collision_test_handler),
            name: "collides",
            api_doc: crate::openapi::ApiDoc {
                method: "GET",
                path: "/my-api-docs",
                operation_id: "collides",
                success_status: 200,
                ..Default::default()
            },
            repository: None,
            idempotency: crate::route::RouteIdempotency::Direct,
        };

        let ctx = RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            openapi: Some(openapi),
        };
        let err = super::try_build_router_inner(vec![user_route], &config, test_state(), ctx)
            .expect_err("user-owned path should prevent OpenAPI mount");
        assert!(matches!(
            err,
            RouterBuildError::OpenApiPathCollision { field: "openapi_json_path", ref path } if path == "/my-api-docs"
        ));
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn try_build_router_rejects_openapi_path_colliding_with_framework_route() {
        let config = AutumnConfig::default(); // /actuator/health is a GET by default
        let openapi = crate::openapi::OpenApiConfig::new("Demo", "1.0.0")
            .openapi_json_path("/actuator/health");
        let ctx = RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            openapi: Some(openapi),
        };
        let err = super::try_build_router_inner(Vec::new(), &config, test_state(), ctx)
            .expect_err("framework-owned path should prevent OpenAPI mount");
        assert!(matches!(
            err,
            RouterBuildError::OpenApiPathCollision {
                field: "openapi_json_path",
                ..
            }
        ));
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn try_build_router_rejects_swagger_ui_asset_path_colliding_with_user_route() {
        let config = AutumnConfig::default();
        let openapi = crate::openapi::OpenApiConfig::new("Demo", "1.0.0");

        let user_route = Route {
            method: http::Method::GET,
            path: "/swagger-ui/swagger-ui.css",
            handler: axum::routing::get(collision_test_handler),
            name: "swagger-ui-asset-collides",
            api_doc: crate::openapi::ApiDoc {
                method: "GET",
                path: "/swagger-ui/swagger-ui.css",
                operation_id: "swagger_ui_asset_collides",
                success_status: 200,
                ..Default::default()
            },
            repository: None,
            idempotency: crate::route::RouteIdempotency::Direct,
        };

        let ctx = RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            openapi: Some(openapi),
        };
        let err = super::try_build_router_inner(vec![user_route], &config, test_state(), ctx)
            .expect_err("swagger ui asset path should be reserved");
        assert!(matches!(
            err,
            RouterBuildError::OpenApiPathCollision {
                field: "swagger_ui_path",
                ref path,
            } if path == "/swagger-ui/swagger-ui.css"
        ));
    }

    #[cfg(all(feature = "openapi", feature = "htmx"))]
    #[tokio::test]
    async fn try_build_router_rejects_openapi_path_colliding_with_htmx_csrf_route() {
        let config = AutumnConfig::default();
        let openapi = crate::openapi::OpenApiConfig::new("Demo", "1.0.0")
            .openapi_json_path(crate::htmx::HTMX_CSRF_JS_PATH);
        let ctx = RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            openapi: Some(openapi),
        };
        let err = super::try_build_router_inner(Vec::new(), &config, test_state(), ctx)
            .expect_err("htmx csrf helper path should be reserved");
        assert!(matches!(
            err,
            RouterBuildError::OpenApiPathCollision {
                field: "openapi_json_path",
                ref path,
            } if path == crate::htmx::HTMX_CSRF_JS_PATH
        ));
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn try_build_router_rejects_openapi_path_under_nest_prefix() {
        // Nesting `/api` means that router owns everything under
        // `/api/...`. Mounting OpenAPI at `/api/docs` would either
        // panic on merge or silently lose one of the routes, so the
        // collision check rejects it.
        let config = AutumnConfig::default();
        let openapi =
            crate::openapi::OpenApiConfig::new("Demo", "1.0.0").openapi_json_path("/api/docs");
        let nested = axum::Router::<AppState>::new()
            .route("/inner", axum::routing::get(|| async { "inner" }));
        let ctx = RouterContext {
            exception_filters: Vec::new(),
            scoped_groups: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: vec![("/api".to_owned(), nested)],
            custom_layers: Vec::new(),
            error_page_renderer: None,
            session_store: None,
            openapi: Some(openapi),
        };
        let err = super::try_build_router_inner(Vec::new(), &config, test_state(), ctx)
            .expect_err("OpenAPI path under a nest prefix should collide");
        assert!(matches!(
            err,
            RouterBuildError::OpenApiPathCollision {
                field: "openapi_json_path",
                ref path,
            } if path == "/api/docs"
        ));
    }

    #[cfg(feature = "openapi")]
    #[test]
    fn try_build_router_rejects_openapi_path_on_dev_live_reload() {
        temp_env::with_vars(
            [
                ("AUTUMN_DEV_RELOAD", Some("1")),
                ("AUTUMN_DEV_RELOAD_STATE", Some("/tmp/autumn-reload-test")),
            ],
            || {
                let config = AutumnConfig::default();
                let openapi = crate::openapi::OpenApiConfig::new("Demo", "1.0.0")
                    .openapi_json_path("/__autumn/live-reload");
                let ctx = RouterContext {
                    exception_filters: Vec::new(),
                    scoped_groups: Vec::new(),
                    merge_routers: Vec::new(),
                    nest_routers: Vec::new(),
                    custom_layers: Vec::new(),
                    error_page_renderer: None,
                    session_store: None,
                    openapi: Some(openapi),
                };
                let err = super::try_build_router_inner(Vec::new(), &config, test_state(), ctx)
                    .expect_err("dev reload path should be reserved");
                assert!(matches!(
                    err,
                    RouterBuildError::OpenApiPathCollision {
                        field: "openapi_json_path",
                        ..
                    }
                ));
            },
        );
    }

    // --- Static file serving (SSG/ISG) tests ---

    fn create_static_dist(revalidate: Option<u64>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(dist.join("about")).expect("mkdir about");
        std::fs::write(dist.join("index.html"), b"<h1>Home</h1>").expect("write index");
        std::fs::write(dist.join("about/index.html"), b"<h1>About</h1>").expect("write about");

        let mut routes = std::collections::HashMap::new();
        routes.insert(
            "/".to_owned(),
            crate::static_gen::ManifestEntry {
                file: "index.html".to_owned(),
                revalidate: None,
            },
        );
        routes.insert(
            "/about".to_owned(),
            crate::static_gen::ManifestEntry {
                file: "about/index.html".to_owned(),
                revalidate,
            },
        );

        let manifest = crate::static_gen::StaticManifest {
            generated_at: "2026-05-18T00:00:00Z".to_owned(),
            autumn_version: "0.4.0".to_owned(),
            routes,
        };
        let json = serde_json::to_string(&manifest).expect("serialize manifest");
        std::fs::write(dist.join("manifest.json"), json).expect("write manifest");
        dir
    }

    #[tokio::test]
    async fn static_serving_serves_get_request_inside_user_layers() {
        let tmp = create_static_dist(None);
        let dist = tmp.path().join("dist");
        let config = AutumnConfig::default();

        let router = try_build_router_with_static(Vec::new(), &config, test_state(), Some(&dist))
            .expect("router builds");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"<h1>About</h1>");
    }

    #[tokio::test]
    async fn static_serving_serves_head_request() {
        let tmp = create_static_dist(None);
        let dist = tmp.path().join("dist");
        let config = AutumnConfig::default();

        let router = try_build_router_with_static(Vec::new(), &config, test_state(), Some(&dist))
            .expect("router builds");

        let response = router
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/about")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty(), "HEAD response body should be empty");
    }

    #[tokio::test]
    async fn static_serving_normalizes_trailing_slash() {
        let tmp = create_static_dist(None);
        let dist = tmp.path().join("dist");
        let config = AutumnConfig::default();

        let router = try_build_router_with_static(Vec::new(), &config, test_state(), Some(&dist))
            .expect("router builds");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/about/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn static_serving_falls_through_for_unknown_route() {
        let tmp = create_static_dist(None);
        let dist = tmp.path().join("dist");
        let config = AutumnConfig::default();

        let router = try_build_router_with_static(Vec::new(), &config, test_state(), Some(&dist))
            .expect("router builds");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/not-in-manifest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn static_serving_skipped_when_no_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).expect("mkdir dist");
        let config = AutumnConfig::default();

        let router = try_build_router_with_static(Vec::new(), &config, test_state(), Some(&dist))
            .expect("router builds even without manifest");

        let response = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn static_serving_with_isr_manifest_builds_successfully() {
        let tmp = create_static_dist(Some(3600));
        let dist = tmp.path().join("dist");
        let config = AutumnConfig::default();

        let router = try_build_router_with_static(Vec::new(), &config, test_state(), Some(&dist))
            .expect("router with ISR manifest should build");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[cfg(test)]
mod trusted_host_tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn trusted_host_allows_matching_and_blocks_nonmatching() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into(), ".example.com".into()];
        let state = crate::state::AppState::for_test();
        let router = build_router(vec![], &cfg, state);

        let ok = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "api.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::NOT_FOUND);

        let blocked = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trusted_host_wildcard_allows_any_host() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["*".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "anything.example")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trusted_host_bypasses_probe_paths() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/actuator/health")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trusted_host_bypasses_actuator_health_path() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/actuator/health")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trusted_host_release_rejects_loopback_unless_listed() {
        let mut cfg = AutumnConfig {
            profile: Some("prod".into()),
            ..AutumnConfig::default()
        };
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trusted_host_uses_uri_authority_when_host_header_missing() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("http://EXAMPLE.COM/nope")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trusted_host_accepts_bracketed_ipv6_loopback_in_dev() {
        let cfg = AutumnConfig::default();
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "[::1]:3000")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trusted_host_matching_is_case_insensitive() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "EXAMPLE.COM")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trusted_host_rejects_malformed_port() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "example.com:abc")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trusted_host_rejects_empty_port_suffix() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "example.com:")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trusted_host_rejects_bracketed_reg_name() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "[example.com]")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    #[tokio::test]
    async fn trusted_host_configured_trailing_dot_matches_normalized_host() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com.".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "example.com")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trusted_host_accepts_trailing_dot_fqdn() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header("host", "example.com.")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trusted_host_bypasses_custom_probe_path_only() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        cfg.health.path = "/healthz".into();
        cfg.health.startup_path = "/startupz".into();
        cfg.health.ready_path = "/readyz".into();
        cfg.health.live_path = "/livez".into();
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());

        let bypassed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(bypassed.status(), StatusCode::OK);

        let not_bypassed = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(not_bypassed.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trusted_host_does_not_bypass_non_get_probe_path_requests() {
        let mut cfg = AutumnConfig::default();
        cfg.security.trusted_hosts.hosts = vec!["example.com".into()];
        let router = build_router(vec![], &cfg, crate::state::AppState::for_test());
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/health")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
#[derive(Clone, Debug)]
struct TrustedHostPolicy {
    rules: Arc<Vec<String>>,
    allow_any: bool,
    allow_missing_host: bool,
    probe_bypass_paths: Arc<std::collections::HashSet<String>>,
}

impl TrustedHostPolicy {
    fn from_config(config: &AutumnConfig) -> Self {
        let mut rules: Vec<String> = config
            .security
            .trusted_hosts
            .hosts
            .iter()
            .map(|h| h.trim().to_ascii_lowercase())
            .map(|h| h.trim_end_matches('.').to_owned())
            .filter(|h| !h.is_empty())
            .collect();
        let is_production = matches!(config.profile.as_deref(), Some("prod" | "production"));
        if !is_production {
            rules.extend(
                ["localhost", "127.0.0.1", "::1"]
                    .into_iter()
                    .map(std::borrow::ToOwned::to_owned),
            );
        }
        let allow_any = rules.iter().any(|h| h == "*");
        let probe_bypass_paths = std::collections::HashSet::from([
            config.health.path.clone(),
            config.health.live_path.clone(),
            config.health.ready_path.clone(),
            config.health.startup_path.clone(),
            crate::actuator::actuator_route_path(&config.actuator.prefix, "/health"),
        ]);
        Self {
            rules: Arc::new(rules),
            allow_any,
            allow_missing_host: !is_production,
            probe_bypass_paths: Arc::new(probe_bypass_paths),
        }
    }

    fn allows_host(&self, host: &str) -> bool {
        if self.allow_any {
            return true;
        }
        self.rules.iter().any(|rule| {
            rule.strip_prefix('.').map_or_else(
                || host == rule,
                |suffix| {
                    host == suffix
                        || host
                            .strip_suffix(suffix)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                },
            )
        })
    }
}
