//! Router construction and configuration.
//!
//! This module handles assembling the final [`axum::Router`] from the various
//! components configured in [`AppBuilder`](crate::app::AppBuilder), including
//! user routes, static files, middleware, error pages, and framework endpoints
//! like actuators and probes.

use std::sync::Arc;

use crate::app::ScopedGroup;
use crate::config::AutumnConfig;
use crate::error_pages::{self, SharedRenderer};
use crate::middleware::RequestIdLayer;
use crate::middleware::dev;
use crate::middleware::exception_filter::{ExceptionFilter, ExceptionFilterLayer};
use crate::route::Route;
use crate::state::AppState;
use axum::extract::State;
use axum::middleware::Next;
use axum::response::IntoResponse;
use http::StatusCode;
use thiserror::Error;

/// Errors that can occur during the router build process.
///
/// These errors are typically fatal and represent configuration or routing
/// definition issues that must be fixed before the application can start.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RouterBuildError {
    /// The session backend configuration is invalid (e.g. Redis without a URL).
    #[error("invalid session backend configuration: {0}")]
    InvalidSessionBackend(#[from] crate::session::SessionBackendConfigError),
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
    pub error_page_renderer: Option<SharedRenderer>,
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
            error_page_renderer: None,
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
            error_page_renderer: None,
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
    let mut router = group_and_mount_routes(route_list);

    let dev_reload_enabled = dev::is_enabled_with_env(&crate::config::OsEnv);

    router = mount_framework_routes(router, dev_reload_enabled);

    let (mounted_probe_paths, router_with_probes) = mount_probe_endpoints(router, config);
    router = router_with_probes;

    router = mount_actuator_endpoints(router, config, &mounted_probe_paths)?;

    // Static file serving from project's static/ directory.
    let env = crate::config::OsEnv;
    let static_dir = crate::app::project_dir("static", &env);
    router = router.nest_service("/static", tower_http::services::ServeDir::new(&static_dir));

    router = mount_scoped_groups(router, ctx.scoped_groups);

    router = mount_raw_routers(router, ctx.merge_routers, ctx.nest_routers);

    router = apply_middleware(
        router,
        config,
        &state,
        ctx.exception_filters,
        ctx.error_page_renderer,
    )?;

    if dev_reload_enabled {
        router = router
            .layer(axum::middleware::from_fn(dev::disable_static_cache))
            .layer(axum::middleware::from_fn(dev::inject_live_reload));
    }

    Ok(router.with_state(state))
}

fn group_and_mount_routes(route_list: Vec<Route>) -> axum::Router<AppState> {
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
        grouped
            .entry(route.path)
            .and_modify(|existing| {
                *existing = std::mem::take(existing).merge(route.handler.clone());
            })
            .or_insert(route.handler);
    }

    let mut router = axum::Router::new();
    for (path, method_router) in grouped {
        router = router.route(path, method_router);
    }
    router
}

fn mount_framework_routes(
    mut router: axum::Router<AppState>,
    dev_reload_enabled: bool,
) -> axum::Router<AppState> {
    // Framework-provided routes
    #[cfg(feature = "htmx")]
    {
        router = router.route("/static/js/htmx.min.js", axum::routing::get(htmx_handler));
        tracing::debug!(
            method = "GET",
            path = "/static/js/htmx.min.js",
            name = format!("htmx {}", crate::htmx::HTMX_VERSION),
            "Mounted route"
        );
    }

    if dev_reload_enabled {
        router = router.route(
            dev::LIVE_RELOAD_PATH,
            axum::routing::get(dev::live_reload_state_handler),
        );
        tracing::debug!(
            path = dev::LIVE_RELOAD_PATH,
            "Mounted dev live reload endpoint"
        );
    }

    router
}

fn mount_probe_endpoints(
    mut router: axum::Router<AppState>,
    config: &AutumnConfig,
) -> (std::collections::HashSet<String>, axum::Router<AppState>) {
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
            axum::routing::get(crate::health::handler),
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
            sub_router = sub_router.route(route.path, route.handler);
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
) -> axum::Router<AppState> {
    // Merge user-supplied raw Axum routers (escape hatch).
    // Merged after annotated routes so annotated routes take precedence.
    for raw_router in merge_routers {
        tracing::debug!("Merged raw Axum router");
        router = router.merge(raw_router);
    }

    // Nest user-supplied raw Axum routers under path prefixes.
    for (prefix, raw_router) in nest_routers {
        tracing::debug!(prefix = %prefix, "Nested raw Axum router");
        router = router.nest(&prefix, raw_router);
    }
    router
}

fn apply_cors_middleware(
    mut router: axum::Router<AppState>,
    config: &AutumnConfig,
) -> axum::Router<AppState> {
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

fn apply_csrf_middleware(
    mut router: axum::Router<AppState>,
    config: &AutumnConfig,
) -> axum::Router<AppState> {
    // CSRF middleware (only applied when enabled)
    if config.security.csrf.enabled {
        let csrf_layer = crate::security::CsrfLayer::from_config(&config.security.csrf);
        tracing::info!("CSRF protection enabled");
        router = router.layer(csrf_layer);
    }
    router
}

fn apply_middleware(
    mut router: axum::Router<AppState>,
    config: &AutumnConfig,
    state: &AppState,
    exception_filters: Vec<Arc<dyn ExceptionFilter>>,
    error_page_renderer: Option<SharedRenderer>,
) -> Result<axum::Router<AppState>, RouterBuildError> {
    router = apply_cors_middleware(router, config);
    router = apply_csrf_middleware(router, config);

    // Security headers layer (always applied)
    let security_headers =
        crate::security::SecurityHeadersLayer::from_config(&config.security.headers);
    tracing::debug!("Security headers enabled");

    // 404 fallback handler for unmatched routes
    router = router.fallback(crate::middleware::error_page_filter::fallback_404_handler);

    // Apply framework middleware. Exception filters wrap outermost so they
    // see all error responses regardless of scoping or interceptors.
    let router = router.layer(RequestIdLayer).layer(security_headers);
    let router =
        crate::session::apply_session_layer(router, &config.session, config.profile.as_deref())?;
    tracing::debug!(backend = ?config.session.backend, "Session management enabled");

    // Error page filter: renders HTML error pages for browser requests.
    // Always registered (uses default renderer if no custom one is provided).
    let is_dev = config
        .profile
        .as_deref()
        .map_or(cfg!(debug_assertions), |p| p == "dev");
    let renderer = error_page_renderer.unwrap_or_else(error_pages::default_renderer);
    let error_page_filter =
        crate::middleware::error_page_filter::ErrorPageFilter { renderer, is_dev };

    // Combine the error page filter with user exception filters.
    // The error page filter runs first (innermost), then user filters.
    let mut all_filters: Vec<Arc<dyn ExceptionFilter>> = vec![Arc::new(error_page_filter)];
    all_filters.extend(exception_filters);

    let count = all_filters.len();
    tracing::debug!(
        count,
        "Registered exception filters (including error page filter)"
    );

    // Error page context layer must be inner to the exception filter so
    // WantsHtml is set on the response before the filter inspects it.
    // Layer order: Metrics -> ExceptionFilter -> ErrorPageContext -> router
    let router = router
        .layer(crate::middleware::error_page_filter::ErrorPageContextLayer)
        .layer(ExceptionFilterLayer::new(all_filters))
        .layer(crate::middleware::MetricsLayer::new(state.metrics.clone()));

    Ok(router)
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
            error_page_renderer: None,
        },
    )
}

pub fn try_build_router_with_static_inner(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    dist_dir: Option<&std::path::Path>,
    ctx: RouterContext,
) -> Result<axum::Router, RouterBuildError> {
    let startup_barrier_state = state.clone();
    let app_router = try_build_router_inner(route_list, config, state, ctx)?;

    let Some(dist) = dist_dir else {
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
        return Ok(apply_startup_barrier(
            app_router,
            config,
            &startup_barrier_state,
        ));
    };

    // Enable ISR regeneration by attaching the app router to the static layer.
    // Routes with `revalidate` set will spawn background re-render tasks
    // when their files become stale.
    let has_isr = layer
        .manifest()
        .routes
        .values()
        .any(|e| e.revalidate.is_some());
    let layer = if has_isr {
        layer.with_router(app_router.clone())
    } else {
        layer
    };

    for (route, entry) in &layer.manifest().routes {
        tracing::debug!(
            route = %route,
            file = %entry.file,
            revalidate = ?entry.revalidate,
            "Static route"
        );
    }

    // Store the layer in an Arc so the static-first middleware can use it.
    let layer = Arc::new(layer);

    // Static-first serving: intercept GET/HEAD requests whose path appears
    // in the manifest and serve pre-built HTML directly, BEFORE the dynamic
    // router runs.  This matches Next.js SSG/ISR semantics where static
    // pages always win over dynamic handlers.
    //
    // Requests not in the manifest (including non-GET/HEAD methods) fall
    // through to the dynamic router unchanged.
    //
    // ISR staleness checking happens inside `resolve()`: stale pages are
    // still served immediately while background regeneration runs
    // (stale-while-revalidate).
    let static_layer = layer;
    let router = app_router.layer(axum::middleware::from_fn(
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
                    if let Some(file_path) = static_layer.resolve(normalized) {
                        if let Ok(contents) = tokio::fs::read(&file_path).await {
                            let body = if is_head {
                                axum::body::Body::empty()
                            } else {
                                axum::body::Body::from(contents)
                            };
                            return http::Response::builder()
                                .status(http::StatusCode::OK)
                                .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                                .body(body)
                                .unwrap();
                        }
                    }
                }
                next.run(req).await
            }
        },
    ));

    Ok(apply_startup_barrier(
        router,
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
    router.layer(axum::middleware::from_fn_with_state(
        barrier_state,
        startup_barrier,
    ))
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
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new().allow_origin(origins)
    };

    let methods: Vec<http::Method> = cors
        .allowed_methods
        .iter()
        .filter_map(|m| m.parse().ok())
        .collect();

    let headers: Vec<HeaderName> = cors
        .allowed_headers
        .iter()
        .filter_map(|h| h.parse().ok())
        .collect();

    layer
        .allow_methods(methods)
        .allow_headers(headers)
        .allow_credentials(cors.allow_credentials)
        .max_age(std::time::Duration::from_secs(cors.max_age_secs))
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
            profile: Some("test".to_owned()),
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
    async fn apply_csrf_middleware_skipped_when_disabled() {
        let config = AutumnConfig::default();
        assert!(!config.security.csrf.enabled);

        let base: axum::Router<AppState> =
            axum::Router::new().route("/form", axum::routing::post(|| async { "posted" }));
        let router = apply_csrf_middleware(base, &config).with_state(test_state());

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
    async fn apply_csrf_middleware_blocks_without_token_when_enabled() {
        let mut config = AutumnConfig::default();
        config.security.csrf.enabled = true;

        let base: axum::Router<AppState> =
            axum::Router::new().route("/form", axum::routing::post(|| async { "posted" }));
        let router = apply_csrf_middleware(base, &config).with_state(test_state());

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
}
