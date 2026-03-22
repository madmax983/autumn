//! Application builder — the entry point for configuring and running
//! an Autumn server.
//!
//! # Example
//!
//! ```ignore
//! use autumn::{get, routes};
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
use crate::db;
use crate::middleware::RequestIdLayer;
use crate::route::Route;

/// Create a new application builder.
#[must_use]
pub const fn app() -> AppBuilder {
    AppBuilder { routes: Vec::new() }
}

/// Builder for configuring and launching an Autumn application.
///
/// Collect routes with [`.routes()`](Self::routes), then call
/// [`.run()`](Self::run) to start the server.
pub struct AppBuilder {
    routes: Vec<Route>,
}

impl AppBuilder {
    /// Add a collection of routes to the application.
    ///
    /// Can be called multiple times — routes are combined.
    ///
    /// ```ignore
    /// autumn::app()
    ///     .routes(users::routes())
    ///     .routes(posts::routes())
    ///     .run()
    ///     .await;
    /// ```
    #[must_use]
    pub fn routes(mut self, routes: Vec<Route>) -> Self {
        self.routes.extend(routes);
        self
    }

    /// Start the HTTP server.
    ///
    /// This method:
    /// 1. Loads configuration from `autumn.toml` (or defaults)
    /// 2. Validates that at least one route is registered
    /// 3. Creates the database pool (if configured)
    /// 4. Builds the Axum router from collected routes
    /// 5. Binds to the configured address and port
    /// 6. Serves requests with graceful shutdown on Ctrl+C
    ///
    /// # Panics
    ///
    /// Panics if no routes are registered. This is a developer error —
    /// call `.routes()` before `.run()`.
    pub async fn run(self) {
        // 1. Load configuration
        let config = AutumnConfig::load().unwrap_or_else(|e| {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        });

        // 2. Validate routes
        assert!(
            !self.routes.is_empty(),
            "No routes registered. Did you forget to call .routes()?"
        );

        // 3. Log banner
        println!("Autumn v{}", env!("CARGO_PKG_VERSION"));

        // 4. Create database pool (if configured)
        let pool = match db::create_pool(&config.database) {
            Ok(pool) => pool,
            Err(e) => {
                eprintln!("Failed to create database pool: {e}");
                std::process::exit(1);
            }
        };

        if pool.is_some() {
            println!(
                "  Database pool: {} max connections",
                config.database.pool_size
            );
        } else {
            println!("  Database: not configured");
        }

        // 5. Build Axum router, logging each route as it mounts
        let mut router = axum::Router::new();
        for route in self.routes {
            println!("  {} {} ({})", route.method, route.path, route.name);
            router = router.route(route.path, route.handler);
        }

        // Framework-provided routes
        router = router.route("/static/js/htmx.min.js", axum::routing::get(htmx_handler));
        println!(
            "  GET /static/js/htmx.min.js (htmx {})",
            crate::htmx::HTMX_VERSION
        );

        // Static file serving from project's static/ directory
        router = router.nest_service("/static", tower_http::services::ServeDir::new("static"));

        let state = AppState { pool };
        let router = router.layer(RequestIdLayer).with_state(state);

        // 6. Bind and serve with graceful shutdown
        let addr = format!("{}:{}", config.server.host, config.server.port);
        println!("Listening on http://{addr}");

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .unwrap_or_else(|e| {
                eprintln!("Failed to bind to {addr}: {e}");
                std::process::exit(1);
            });

        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .unwrap_or_else(|e| {
                eprintln!("Server error: {e}");
                std::process::exit(1);
            });
    }
}

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

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install Ctrl+C handler");
    println!("\nShutting down gracefully...");
}

#[cfg(test)]
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
