use std::sync::Arc;

use crate::app::ScopedGroup;
use crate::config::AutumnConfig;
use crate::error_pages::{self, SharedRenderer};
use crate::middleware::RequestIdLayer;
use crate::middleware::dev;
use crate::middleware::exception_filter::{ExceptionFilter, ExceptionFilterLayer};
use crate::route::Route;
use crate::state::AppState;

/// Build the fully-configured Axum router from routes, config, and state.
///
/// Extracted from `AppBuilder::run` so the router construction logic is
/// testable without binding a real TCP listener.
pub fn build_router(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
) -> axum::Router {
    build_router_inner(
        route_list,
        config,
        state,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
    )
}

/// Build a router that includes user-supplied raw Axum routers.
///
/// Like [`build_router`], but also merges and nests additional raw
/// Axum routers. This is primarily useful for integration testing;
/// in production, use [`AppBuilder::merge`](crate::app::AppBuilder::merge) and [`AppBuilder::nest`](crate::app::AppBuilder::nest).
pub fn build_router_merged(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
) -> axum::Router {
    build_router_inner(
        route_list,
        config,
        state,
        Vec::new(),
        Vec::new(),
        merge_routers,
        nest_routers,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::cognitive_complexity)]
pub(crate) fn build_router_inner(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    exception_filters: Vec<Arc<dyn ExceptionFilter>>,
    scoped_groups: Vec<ScopedGroup>,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
    error_page_renderer: Option<SharedRenderer>,
) -> axum::Router {
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

    let dev_reload_enabled = dev::is_enabled_with_env(&crate::config::OsEnv);

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

    // Health check endpoint (auto-mounted)
    router = router.route(
        &config.health.path,
        axum::routing::get(crate::health::handler),
    );
    tracing::debug!(path = %config.health.path, "Mounted health check");

    // Actuator endpoints
    let actuator_sensitive = config.actuator.sensitive;
    router = router.merge(crate::actuator::actuator_router(actuator_sensitive));
    tracing::debug!(
        sensitive = actuator_sensitive,
        "Mounted actuator endpoints at /actuator/*"
    );

    // Static file serving from project's static/ directory.
    let env = crate::config::OsEnv;
    let static_dir = crate::app::project_dir("static", &env);
    router = router.nest_service("/static", tower_http::services::ServeDir::new(&static_dir));

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

    // CSRF middleware (only applied when enabled)
    if config.security.csrf.enabled {
        let csrf_layer = crate::security::CsrfLayer::from_config(&config.security.csrf);
        tracing::info!("CSRF protection enabled");
        router = router.layer(csrf_layer);
    }

    // Session management layer (always enabled with default in-memory store)
    let session_layer = crate::session::SessionLayer::new(
        crate::session::MemoryStore::new(),
        config.session.clone(),
    );
    tracing::debug!("Session management enabled (in-memory store)");

    // Security headers layer (always applied)
    let security_headers =
        crate::security::SecurityHeadersLayer::from_config(&config.security.headers);
    tracing::debug!("Security headers enabled");

    // 404 fallback handler for unmatched routes
    router = router.fallback(crate::middleware::error_page_filter::fallback_404_handler);

    // Apply framework middleware. Exception filters wrap outermost so they
    // see all error responses regardless of scoping or interceptors.
    let router = router
        .layer(RequestIdLayer)
        .layer(security_headers)
        .layer(session_layer);

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
    let mut router = router
        .layer(crate::middleware::error_page_filter::ErrorPageContextLayer)
        .layer(ExceptionFilterLayer::new(all_filters))
        .layer(crate::middleware::MetricsLayer::new(state.metrics.clone()));

    if dev_reload_enabled {
        router = router
            .layer(axum::middleware::from_fn(dev::disable_static_cache))
            .layer(axum::middleware::from_fn(dev::inject_live_reload));
    }

    router.with_state(state)
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
pub fn build_router_with_static(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    dist_dir: Option<&std::path::Path>,
) -> axum::Router {
    build_router_with_static_inner(
        route_list,
        config,
        state,
        dist_dir,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_router_with_static_inner(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
    dist_dir: Option<&std::path::Path>,
    exception_filters: Vec<Arc<dyn ExceptionFilter>>,
    scoped_groups: Vec<ScopedGroup>,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
    error_page_renderer: Option<SharedRenderer>,
) -> axum::Router {
    let app_router = build_router_inner(
        route_list,
        config,
        state,
        exception_filters,
        scoped_groups,
        merge_routers,
        nest_routers,
        error_page_renderer,
    );

    let Some(dist) = dist_dir else {
        return app_router;
    };

    let Some(layer) = crate::static_gen::StaticFileLayer::new(dist) else {
        tracing::debug!(
            dist = %dist.display(),
            "No valid manifest.json in dist dir; skipping static file layer"
        );
        return app_router;
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
    app_router.layer(axum::middleware::from_fn(
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
    ))
}

/// Build a `tower_http::cors::CorsLayer` from the framework's [`crate::config::CorsConfig`].
///
/// Called only when `config.cors.allowed_origins` is non-empty.
pub(crate) fn build_cors_layer(cors: &crate::config::CorsConfig) -> tower_http::cors::CorsLayer {
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
pub(crate) async fn htmx_handler() -> axum::response::Response {
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
