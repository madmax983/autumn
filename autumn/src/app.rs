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
//! use autumn::prelude::*;
//!
//! #[get("/hello")]
//! async fn hello() -> &'static str { "Hello!" }
//!
//! #[autumn::main]
//! async fn main() {
//!     autumn::app()
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
/// use autumn::prelude::*;
///
/// #[get("/")]
/// async fn index() -> &'static str { "hi" }
///
/// #[autumn::main]
/// async fn main() {
///     autumn::app()
///         .routes(routes![index])
///         .run()
///         .await;
/// }
/// ```
#[must_use]
pub const fn app() -> AppBuilder {
    AppBuilder { routes: Vec::new() }
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
/// use autumn::prelude::*;
///
/// #[get("/a")]
/// async fn route_a() -> &'static str { "a" }
///
/// #[get("/b")]
/// async fn route_b() -> &'static str { "b" }
///
/// #[autumn::main]
/// async fn main() {
///     autumn::app()
///         .routes(routes![route_a])
///         .routes(routes![route_b])
///         .run()
///         .await;
/// }
/// ```
pub struct AppBuilder {
    routes: Vec<Route>,
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
    /// # use autumn::prelude::*;
    /// # #[get("/users")] async fn list_users() -> &'static str { "" }
    /// # #[get("/posts")] async fn list_posts() -> &'static str { "" }
    /// # #[autumn::main]
    /// # async fn main() {
    /// autumn::app()
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
        // 1. Load configuration
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

        // 4. Log banner
        tracing::info!("Autumn v{}", env!("CARGO_PKG_VERSION"));

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

        // 6. Build Axum router, logging each route as it mounts
        let mut router = axum::Router::new();
        for route in self.routes {
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

        // Static file serving from project's static/ directory.
        // Resolve relative to the app's crate root (set by #[autumn::main])
        // so `cargo run -p <example>` works from the workspace root.
        let static_dir = if let Ok(manifest_dir) = std::env::var("AUTUMN_MANIFEST_DIR") {
            std::path::PathBuf::from(manifest_dir).join("static")
        } else {
            std::path::PathBuf::from("static")
        };
        router = router.nest_service("/static", tower_http::services::ServeDir::new(&static_dir));

        let state = AppState {
            #[cfg(feature = "db")]
            pool,
        };
        let router = router.layer(RequestIdLayer).with_state(state);

        // 7. Bind and serve with graceful shutdown
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
#[cfg(feature = "htmx")]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

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
}
