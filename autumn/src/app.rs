//! Application builder -- the entry point for configuring and running
//! an Autumn server.
//!
//! Every Autumn application follows the same pattern:
//!
//! 1. Call [`app()`] to create an [`AppBuilder`].
//! 2. Register routes with [`.routes()`](AppBuilder::routes).
//! 3. Call [`.run()`](AppBuilder::run) to start serving.
//!
//! # Example
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//!
//! #[get("/hello")]
//! async fn hello() -> &'static str { "Hello!" }
//!
//! #[autumn_web::main]
//! async fn main() {
//!     autumn_web::app()
//!         .routes(routes![hello])
//!         .run()
//!         .await;
//! }
//! ```

use crate::AppState;
use crate::config::AutumnConfig;
#[cfg(feature = "db")]
use crate::db;
use crate::middleware::RequestIdLayer;
use crate::route::Route;

/// Create a new [`AppBuilder`].
///
/// This is the primary entry point for constructing an Autumn application.
/// Chain [`.routes()`](AppBuilder::routes) calls to register handlers, then
/// call [`.run()`](AppBuilder::run) to start the server.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/")]
/// async fn index() -> &'static str { "hi" }
///
/// #[autumn_web::main]
/// async fn main() {
///     autumn_web::app()
///         .routes(routes![index])
///         .run()
///         .await;
/// }
/// ```
#[must_use]
pub const fn app() -> AppBuilder {
    AppBuilder {
        routes: Vec::new(),
        tasks: Vec::new(),
    }
}

/// Builder for configuring and launching an Autumn application.
///
/// Created by [`app()`]. Collect routes with [`.routes()`](Self::routes),
/// then call [`.run()`](Self::run) to start the HTTP server.
///
/// The builder follows the **builder pattern**: each method consumes `self`
/// and returns a new `AppBuilder`, allowing chained calls.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/a")]
/// async fn route_a() -> &'static str { "a" }
///
/// #[get("/b")]
/// async fn route_b() -> &'static str { "b" }
///
/// #[autumn_web::main]
/// async fn main() {
///     autumn_web::app()
///         .routes(routes![route_a])
///         .routes(routes![route_b])
///         .run()
///         .await;
/// }
/// ```
pub struct AppBuilder {
    routes: Vec<Route>,
    tasks: Vec<crate::task::TaskInfo>,
}

impl AppBuilder {
    /// Register a collection of routes with the application.
    ///
    /// Can be called multiple times -- routes are combined additively.
    /// Use the [`routes!`](crate::routes) macro to collect annotated
    /// handlers into the expected `Vec<Route>`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use autumn_web::prelude::*;
    /// # #[get("/users")] async fn list_users() -> &'static str { "" }
    /// # #[get("/posts")] async fn list_posts() -> &'static str { "" }
    /// # #[autumn_web::main]
    /// # async fn main() {
    /// autumn_web::app()
    ///     .routes(routes![list_users])
    ///     .routes(routes![list_posts])
    ///     .run()
    ///     .await;
    /// # }
    /// ```
    #[must_use]
    pub fn routes(mut self, routes: Vec<Route>) -> Self {
        self.routes.extend(routes);
        self
    }

    /// Register scheduled background tasks with the application.
    ///
    /// Tasks run alongside the HTTP server and are stopped during
    /// graceful shutdown. Use the [`tasks!`](crate::tasks) macro
    /// to collect `#[scheduled]` handlers.
    #[must_use]
    pub fn tasks(mut self, tasks: Vec<crate::task::TaskInfo>) -> Self {
        self.tasks.extend(tasks);
        self
    }

    /// Start the HTTP server.
    ///
    /// This method performs the full application lifecycle:
    ///
    /// 1. Loads configuration from `autumn.toml` (or defaults).
    /// 2. Initializes the tracing subscriber.
    /// 3. Validates that at least one route is registered.
    /// 4. Creates the database connection pool (if configured).
    /// 5. Builds the Axum router from collected routes.
    /// 6. Mounts built-in routes (health check, htmx JS, static files).
    /// 7. Binds to the configured address and port.
    /// 8. Serves requests with graceful shutdown on Ctrl+C (or `SIGTERM`
    ///    on Unix).
    ///
    /// # Panics
    ///
    /// Panics if no routes have been registered via [`.routes()`](Self::routes).
    /// This is intentional -- an application with no routes is always a
    /// developer error.
    pub async fn run(self) {
        // 1. Load configuration (profile-aware)
        let config = AutumnConfig::load().unwrap_or_else(|e| {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        });

        // 2. Initialize logging immediately after config (before any tracing calls)
        crate::logging::init(&config.log);

        // 3. Validate routes
        assert!(
            !self.routes.is_empty(),
            "No routes registered. Did you forget to call .routes()?"
        );

        // 4. Log banner with profile info
        let profile_display = config.profile.as_deref().unwrap_or("none");
        tracing::info!(
            version = env!("CARGO_PKG_VERSION"),
            profile = profile_display,
            "Autumn starting"
        );

        // 5. Create database pool (if configured)
        #[cfg(feature = "db")]
        let pool = match db::create_pool(&config.database) {
            Ok(pool) => pool,
            Err(e) => {
                tracing::error!("Failed to create database pool: {e}");
                std::process::exit(1);
            }
        };

        #[cfg(feature = "db")]
        if pool.is_some() {
            tracing::info!(
                max_connections = config.database.pool_size,
                "Database pool configured"
            );
        } else {
            tracing::info!("Database not configured");
        }

        // 6. Build the router
        let state = AppState {
            #[cfg(feature = "db")]
            pool,
            profile: config.profile.clone(),
            started_at: std::time::Instant::now(),
            health_detailed: config.health.detailed,
        };
        let router = build_router(self.routes, &config, state.clone());

        // 7. Start scheduled tasks (if any)
        if !self.tasks.is_empty() {
            tracing::info!(count = self.tasks.len(), "Starting scheduled tasks");
            for task_info in &self.tasks {
                let schedule_desc = match &task_info.schedule {
                    crate::task::Schedule::FixedDelay(d) => format!("every {}s", d.as_secs()),
                    crate::task::Schedule::Cron { expression, .. } => {
                        format!("cron {expression}")
                    }
                };
                tracing::info!(name = %task_info.name, schedule = %schedule_desc, "Registered task");
            }
            start_task_scheduler(self.tasks, &state);
        }

        // 8. Bind and serve with graceful shutdown
        let addr = format!("{}:{}", config.server.host, config.server.port);
        tracing::info!(addr = %addr, "Listening");

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(addr = %addr, "Failed to bind: {e}");
                std::process::exit(1);
            });

        let shutdown_timeout = config.server.shutdown_timeout_secs;

        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                shutdown_signal().await;

                // Warn if draining takes too long
                if shutdown_timeout > 5 {
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(
                            shutdown_timeout.saturating_sub(5),
                        ))
                        .await;
                        tracing::warn!(
                            timeout_secs = shutdown_timeout,
                            "Shutdown draining near timeout, force-kill may be imminent"
                        );
                    });
                }
            })
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Server error: {e}");
                std::process::exit(1);
            });

        tracing::info!("Server shut down cleanly");
    }
}

/// Start scheduled tasks in background Tokio tasks.
///
/// Each task runs in its own spawned task with error logging.
/// Uses simple `tokio::time` for fixed-delay scheduling.
fn start_task_scheduler(tasks: Vec<crate::task::TaskInfo>, state: &AppState) {
    for task_info in tasks {
        let state = state.clone();
        let name = task_info.name.clone();
        let handler = task_info.handler;

        match task_info.schedule {
            crate::task::Schedule::FixedDelay(delay) => {
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(delay).await;
                        tracing::debug!(task = %name, "Running scheduled task");
                        match (handler)(state.clone()).await {
                            Ok(()) => tracing::debug!(task = %name, "Task completed"),
                            Err(e) => tracing::warn!(task = %name, error = %e, "Task failed"),
                        }
                    }
                });
            }
            crate::task::Schedule::Cron { expression, .. } => {
                tracing::info!(
                    task = %name,
                    cron = %expression,
                    "Cron scheduling not yet implemented; task registered but will not run"
                );
            }
        }
    }
}

/// Build the fully-configured Axum router from routes, config, and state.
///
/// Extracted from `AppBuilder::run` so the router construction logic is
/// testable without binding a real TCP listener.
pub(crate) fn build_router(
    route_list: Vec<Route>,
    config: &AutumnConfig,
    state: AppState,
) -> axum::Router {
    let mut router = axum::Router::new();
    for route in route_list {
        tracing::debug!(
            method = %route.method,
            path = route.path,
            name = route.name,
            "Mounted route"
        );
        router = router.route(route.path, route.handler);
    }

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
    let static_dir = std::env::var("AUTUMN_MANIFEST_DIR").map_or_else(
        |_| std::path::PathBuf::from("static"),
        |manifest_dir| std::path::PathBuf::from(manifest_dir).join("static"),
    );
    router = router.nest_service("/static", tower_http::services::ServeDir::new(&static_dir));

    router.layer(RequestIdLayer).with_state(state)
}

#[cfg(feature = "htmx")]
async fn htmx_handler() -> axum::response::Response {
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

/// Wait for a shutdown signal (Ctrl+C or SIGTERM on Unix).
///
/// Returns when either signal is received. Axum's `with_graceful_shutdown`
/// then stops accepting new connections and drains in-flight requests.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        tracing::info!("Received Ctrl+C, starting graceful shutdown");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
        tracing::info!("Received SIGTERM, starting graceful shutdown");
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Helper to build a test router with default config and no database.
    fn test_router(routes: Vec<Route>) -> axum::Router {
        let config = AutumnConfig::default();
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
        };
        build_router(routes, &config, state)
    }

    /// Helper to create a simple GET route for testing.
    fn test_get_route(path: &'static str, name: &'static str) -> Route {
        Route {
            method: http::Method::GET,
            path,
            handler: axum::routing::get(|| async { "ok" }),
            name,
        }
    }

    #[tokio::test]
    async fn build_router_mounts_user_routes() {
        let router = test_router(vec![test_get_route("/test", "test_handler")]);

        let response = router
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn build_router_mounts_health_check_at_default_path() {
        let router = test_router(vec![test_get_route("/dummy", "dummy")]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn build_router_mounts_health_check_at_custom_path() {
        let mut config = AutumnConfig::default();
        config.health.path = "/healthz".to_owned();
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
        };
        let router = build_router(vec![test_get_route("/dummy", "dummy")], &config, state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn build_router_adds_request_id_header() {
        let router = test_router(vec![test_get_route("/test", "test")]);

        let response = router
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(response.headers().contains_key("x-request-id"));
    }

    #[tokio::test]
    async fn build_router_unknown_route_returns_404() {
        let router = test_router(vec![test_get_route("/exists", "exists")]);

        let response = router
            .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn build_router_multiple_routes() {
        let router = test_router(vec![test_get_route("/a", "a"), test_get_route("/b", "b")]);

        let resp_a = router
            .clone()
            .oneshot(Request::builder().uri("/a").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK);

        let resp_b = router
            .oneshot(Request::builder().uri("/b").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp_b.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn build_router_post_route() {
        let post_routes = vec![Route {
            method: http::Method::POST,
            path: "/submit",
            handler: axum::routing::post(|| async { "posted" }),
            name: "submit",
        }];
        let config = AutumnConfig::default();
        let state = AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
        };
        let router = build_router(post_routes, &config, state);

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/submit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg(feature = "htmx")]
    #[tokio::test]
    async fn htmx_handler_returns_javascript_with_correct_headers() {
        let app =
            axum::Router::new().route("/static/js/htmx.min.js", axum::routing::get(htmx_handler));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/static/js/htmx.min.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("application/javascript"),
            "Expected application/javascript, got {content_type}"
        );

        let cache_control = response
            .headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            cache_control.contains("immutable"),
            "Expected immutable cache, got {cache_control}"
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        // Body length matches the embedded file
        assert_eq!(body.len(), crate::htmx::HTMX_JS.len());

        // Body starts with valid JavaScript
        let start = std::str::from_utf8(&body[..50]).expect("htmx should be valid UTF-8");
        assert!(
            start.contains("htmx") || start.contains("function"),
            "Response doesn't look like htmx JavaScript: {start}"
        );
    }

    #[cfg(feature = "htmx")]
    #[tokio::test]
    async fn build_router_serves_htmx_js() {
        let router = test_router(vec![test_get_route("/dummy", "dummy")]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/static/js/htmx.min.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let ct = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("javascript"));
    }
}
