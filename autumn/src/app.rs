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
    /// 3. Builds the Axum router from collected routes
    /// 4. Binds to the configured address and port
    /// 5. Serves requests with graceful shutdown on Ctrl+C
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

        // 4. Build Axum router, logging each route as it mounts
        let mut router = axum::Router::new();
        for route in self.routes {
            println!("  {} {} ({})", route.method, route.path, route.name);
            router = router.route(route.path, route.handler);
        }
        let router = router.layer(RequestIdLayer).with_state(AppState);

        // 5. Bind and serve with graceful shutdown
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

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install Ctrl+C handler");
    println!("\nShutting down gracefully...");
}
