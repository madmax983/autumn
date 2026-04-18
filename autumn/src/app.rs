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

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::config::AutumnConfig;
use crate::error_pages::{ErrorPageRenderer, SharedRenderer};
use crate::middleware::exception_filter::ExceptionFilter;
#[cfg(feature = "db")]
use crate::migrate;
use crate::route::Route;
use crate::state::AppState;

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
pub fn app() -> AppBuilder {
    AppBuilder {
        routes: Vec::new(),
        tasks: Vec::new(),
        static_metas: Vec::new(),
        exception_filters: Vec::new(),
        scoped_groups: Vec::new(),
        merge_routers: Vec::new(),
        nest_routers: Vec::new(),
        startup_hooks: Vec::new(),
        shutdown_hooks: Vec::new(),
        extensions: HashMap::new(),
        error_page_renderer: None,
        #[cfg(feature = "db")]
        migrations: Vec::new(),
    }
}

type StartupHookFuture = Pin<Box<dyn Future<Output = crate::AutumnResult<()>> + Send>>;
type StartupHook = Box<dyn Fn(AppState) -> StartupHookFuture + Send + Sync>;
type ShutdownHookFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type ShutdownHook = Box<dyn Fn() -> ShutdownHookFuture + Send + Sync>;

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
    pub(crate) static_metas: Vec<crate::static_gen::StaticRouteMeta>,
    exception_filters: Vec<Arc<dyn ExceptionFilter>>,
    scoped_groups: Vec<ScopedGroup>,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
    startup_hooks: Vec<StartupHook>,
    shutdown_hooks: Vec<ShutdownHook>,
    extensions: HashMap<TypeId, Box<dyn Any + Send>>,
    /// Custom error page renderer (overrides built-in pages).
    error_page_renderer: Option<SharedRenderer>,
    /// Embedded Diesel migrations, registered via `.migrations()`.
    #[cfg(feature = "db")]
    migrations: Vec<migrate::EmbeddedMigrations>,
}

/// A group of routes sharing a common path prefix and middleware layer.
///
/// Created by [`AppBuilder::scoped`]. The routes are mounted under the
/// prefix with the middleware applied only to this group.
pub(crate) struct ScopedGroup {
    pub(crate) prefix: String,
    pub(crate) routes: Vec<Route>,
    /// Closure that applies the layer to a sub-router.
    pub(crate) apply_layer:
        Box<dyn FnOnce(axum::Router<AppState>) -> axum::Router<AppState> + Send>,
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

    /// Register static route metadata for build-time rendering.
    ///
    /// Use the [`static_routes!`](crate::static_routes) macro to collect
    /// `#[static_get]` handlers' metadata.
    #[must_use]
    pub fn static_routes(mut self, metas: Vec<crate::static_gen::StaticRouteMeta>) -> Self {
        self.static_metas.extend(metas);
        self
    }

    /// Register a global exception filter.
    ///
    /// Exception filters intercept error responses produced by
    /// [`AutumnError`](crate::AutumnError) before they are sent to the
    /// client. Filters run in registration order.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use autumn_web::middleware::{ExceptionFilter, AutumnErrorInfo};
    /// use axum::response::Response;
    ///
    /// struct LogFilter;
    /// impl ExceptionFilter for LogFilter {
    ///     fn filter(&self, error: &AutumnErrorInfo, response: Response) -> Response {
    ///         eprintln!("Error: {}", error.message);
    ///         response
    ///     }
    /// }
    ///
    /// # use autumn_web::prelude::*;
    /// # #[get("/")] async fn index() -> &'static str { "" }
    /// # #[autumn_web::main]
    /// # async fn main() {
    /// autumn_web::app()
    ///     .exception_filter(LogFilter)
    ///     .routes(routes![index])
    ///     .run()
    ///     .await;
    /// # }
    /// ```
    #[must_use]
    pub fn exception_filter(mut self, filter: impl ExceptionFilter) -> Self {
        self.exception_filters.push(Arc::new(filter));
        self
    }

    /// Register a custom error page renderer.
    ///
    /// The renderer replaces the built-in default error pages (404, 422, 500,
    /// and generic errors). Implement [`ErrorPageRenderer`] to provide your
    /// own branded error pages.
    ///
    /// Only one renderer can be active. Calling this method multiple times
    /// replaces the previous renderer.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use autumn_web::error_pages::{ErrorPageRenderer, ErrorContext};
    /// use maud::{Markup, html};
    ///
    /// struct MyErrors;
    ///
    /// impl ErrorPageRenderer for MyErrors {
    ///     fn render_error(&self, ctx: &ErrorContext) -> Markup {
    ///         html! {
    ///             h1 { (ctx.status.as_u16()) " - Custom error page" }
    ///         }
    ///     }
    /// }
    ///
    /// # use autumn_web::prelude::*;
    /// # #[get("/")] async fn index() -> &'static str { "" }
    /// # #[autumn_web::main]
    /// # async fn main() {
    /// autumn_web::app()
    ///     .error_pages(MyErrors)
    ///     .routes(routes![index])
    ///     .run()
    ///     .await;
    /// # }
    /// ```
    #[must_use]
    pub fn error_pages(mut self, renderer: impl ErrorPageRenderer) -> Self {
        self.error_page_renderer = Some(Arc::new(renderer));
        self
    }

    /// Register a group of routes with a shared path prefix and middleware.
    ///
    /// The `layer` is applied only to routes within this group, not to the
    /// rest of the application. The routes are mounted under `prefix`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use autumn_web::prelude::*;
    /// use autumn_web::middleware::RequestIdLayer; // any Tower Layer
    ///
    /// # #[get("/")]  async fn index() -> &'static str { "" }
    /// # #[get("/users")] async fn list_users() -> &'static str { "" }
    /// # #[autumn_web::main]
    /// # async fn main() {
    /// autumn_web::app()
    ///     .routes(routes![index])
    ///     .scoped("/api", RequestIdLayer, routes![list_users])
    ///     .run()
    ///     .await;
    /// # }
    /// ```
    #[must_use]
    pub fn scoped<L>(mut self, prefix: &str, layer: L, routes: Vec<Route>) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<
                axum::http::Request<axum::body::Body>,
                Response = axum::http::Response<axum::body::Body>,
                Error = std::convert::Infallible,
            > + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<axum::http::Request<axum::body::Body>>>::Future:
            Send + 'static,
    {
        self.scoped_groups.push(ScopedGroup {
            prefix: prefix.to_owned(),
            routes,
            apply_layer: Box::new(move |router| router.layer(layer)),
        });
        self
    }

    /// Merge a raw Axum router into the application.
    ///
    /// This is an escape hatch for when Autumn's route macros are not
    /// sufficient -- for example, when integrating a third-party Axum
    /// middleware crate or mounting a hand-built WebSocket handler.
    ///
    /// The merged router shares the same [`AppState`] (database pool,
    /// config, etc.) and Autumn's global middleware (request IDs,
    /// security headers, session management) applies to its routes.
    ///
    /// Merged routes are added **after** Autumn's annotated routes, so
    /// if both define the same path, the annotated route takes precedence.
    ///
    /// Can be called multiple times -- routers are accumulated.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use autumn_web::prelude::*;
    /// use autumn_web::AppState;
    ///
    /// #[get("/")]
    /// async fn index() -> &'static str { "hi" }
    ///
    /// #[autumn_web::main]
    /// async fn main() {
    ///     let raw = axum::Router::<AppState>::new()
    ///         .route("/ws", axum::routing::get(|| async { "websocket" }));
    ///
    ///     autumn_web::app()
    ///         .routes(routes![index])
    ///         .merge(raw)
    ///         .run()
    ///         .await;
    /// }
    /// ```
    #[must_use]
    pub fn merge(mut self, router: axum::Router<AppState>) -> Self {
        self.merge_routers.push(router);
        self
    }

    /// Mount a raw Axum router under a path prefix.
    ///
    /// This is an escape hatch similar to [`merge`](Self::merge), but the
    /// router's routes are nested under the given `path` prefix. Useful
    /// for mounting a self-contained API version or third-party router.
    ///
    /// The nested router shares the same [`AppState`] and Autumn's global
    /// middleware applies to its routes.
    ///
    /// Can be called multiple times with different prefixes.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use autumn_web::prelude::*;
    /// use autumn_web::AppState;
    ///
    /// #[get("/")]
    /// async fn index() -> &'static str { "hi" }
    ///
    /// #[autumn_web::main]
    /// async fn main() {
    ///     let v2 = axum::Router::<AppState>::new()
    ///         .route("/users", axum::routing::get(|| async { "v2 users" }));
    ///
    ///     autumn_web::app()
    ///         .routes(routes![index])
    ///         .nest("/api/v2", v2)
    ///         .run()
    ///         .await;
    /// }
    /// ```
    #[must_use]
    pub fn nest(mut self, path: &str, router: axum::Router<AppState>) -> Self {
        self.nest_routers.push((path.to_owned(), router));
        self
    }

    /// Register an async startup hook that runs after [`AppState`] exists and
    /// before the server begins accepting requests.
    ///
    /// This is intended for background runtimes that need the fully built app
    /// state, such as workers or pollers that share the database pool.
    #[must_use]
    pub fn on_startup<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(AppState) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = crate::AutumnResult<()>> + Send + 'static,
    {
        self.startup_hooks
            .push(Box::new(move |state| Box::pin(hook(state))));
        self
    }

    /// Register an async shutdown hook that runs during graceful shutdown.
    ///
    /// Hooks execute in reverse registration order so later-added runtimes
    /// shut down before earlier infrastructure they might depend on.
    #[must_use]
    pub fn on_shutdown<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.shutdown_hooks.push(Box::new(move || Box::pin(hook())));
        self
    }

    /// Store or replace a typed builder extension.
    ///
    /// External crates use this to accumulate configuration across fluent
    /// extension-trait calls without Autumn needing to know the concrete type.
    #[must_use]
    pub fn with_extension<T>(mut self, value: T) -> Self
    where
        T: Any + Send + 'static,
    {
        self.extensions.insert(TypeId::of::<T>(), Box::new(value));
        self
    }

    /// Mutate a typed builder extension, inserting a default value first when
    /// the extension has not been registered yet.
    ///
    /// # Panics
    ///
    /// Panics if the internal extension type map is corrupted and the value
    /// stored under `T`'s [`TypeId`] cannot be downcast back to `T`.
    #[must_use]
    pub fn update_extension<T, Init, Update>(mut self, init: Init, update: Update) -> Self
    where
        T: Any + Send + 'static,
        Init: FnOnce() -> T,
        Update: FnOnce(&mut T),
    {
        let type_id = TypeId::of::<T>();
        let entry = self
            .extensions
            .entry(type_id)
            .or_insert_with(|| Box::new(init()));
        let typed = entry
            .downcast_mut::<T>()
            .expect("extension type map corrupted");
        update(typed);
        self
    }

    /// Borrow a typed builder extension if it has been registered.
    #[must_use]
    pub fn extension<T>(&self) -> Option<&T>
    where
        T: Any + Send + 'static,
    {
        self.extensions.get(&TypeId::of::<T>())?.downcast_ref::<T>()
    }

    /// Register embedded Diesel migrations with the application.
    ///
    /// When migrations are registered:
    /// - In **dev** mode, pending migrations run automatically on startup.
    /// - In **prod** mode, pending migrations are logged as warnings but
    ///   not applied -- use `autumn migrate` to apply them explicitly.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
    ///
    /// const MIGRATIONS: EmbeddedMigrations = embed_migrations!();
    ///
    /// #[autumn_web::main]
    /// async fn main() {
    ///     autumn_web::app()
    ///         .routes(routes![...])
    ///         .migrations(MIGRATIONS)
    ///         .run()
    ///         .await;
    /// }
    /// ```
    #[cfg(feature = "db")]
    #[must_use]
    pub fn migrations(mut self, migrations: migrate::EmbeddedMigrations) -> Self {
        self.migrations.push(migrations);
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
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::cognitive_complexity)]
    pub async fn run(self) {
        // ── Build mode ─────────────────────────────────────────────────
        // When AUTUMN_BUILD_STATIC=1, render static routes to dist/ and exit
        // instead of starting the HTTP server. This is triggered by `autumn build`.
        if is_static_build_mode() {
            self.run_build_mode().await;
            return;
        }

        let Self {
            routes,
            tasks,
            static_metas: _,
            exception_filters,
            scoped_groups,
            merge_routers,
            nest_routers,
            startup_hooks,
            shutdown_hooks,
            extensions: _,
            error_page_renderer,
            #[cfg(feature = "db")]
            migrations,
        } = self;

        let all_routes = routes;

        // 1 & 2. Load configuration and initialize logging/telemetry
        let (config, _telemetry_guard) = load_config_and_telemetry();

        // 3. Validate routes
        assert!(
            !all_routes.is_empty(),
            "No routes registered. Did you forget to call .routes()?"
        );

        // 4. Log banner with profile info
        let profile_display = config.profile.as_deref().unwrap_or("none");
        tracing::info!(
            version = env!("CARGO_PKG_VERSION"),
            profile = profile_display,
            "Autumn starting"
        );

        // 4b. Startup transparency log (AUTUMN_SHOW_CONFIG=1 or log level <= DEBUG)
        let show_config = std::env::var("AUTUMN_SHOW_CONFIG").as_deref() == Ok("1");
        if show_config {
            log_startup_transparency(&all_routes, &tasks, &scoped_groups, &config);
        }

        // 5. Create database pool and run migrations (if configured)
        #[cfg(feature = "db")]
        let pool = setup_database(&config, migrations).unwrap_or_else(|e| {
            tracing::error!("{e}");
            std::process::exit(1);
        });

        #[cfg(feature = "db")]
        if pool.is_some() {
            tracing::info!(
                max_connections = config.database.pool_size,
                "Database pool configured"
            );
        } else {
            tracing::info!("Database not configured");
        }

        // 6. Build the router (with optional static-file layer)
        let state = build_state(
            &config,
            #[cfg(feature = "db")]
            pool,
        );
        let env = crate::config::OsEnv;
        let dist_dir = project_dir("dist", &env);
        let dist_ref = if dist_dir.exists() {
            Some(dist_dir.as_path())
        } else {
            None
        };
        let router = crate::router::try_build_router_with_static_inner(
            all_routes,
            &config,
            state.clone(),
            dist_ref,
            crate::router::RouterContext {
                exception_filters,
                scoped_groups,
                merge_routers,
                nest_routers,
                error_page_renderer,
            },
        )
        .unwrap_or_else(|error| {
            tracing::error!(error = %error, "Failed to build router");
            std::process::exit(1);
        });

        // 7. Bind and serve. We start listening before startup hooks finish so
        // `/startup` can honestly report startup progress.
        let addr = format!("{}:{}", config.server.host, config.server.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(addr = %addr, "Failed to bind: {e}");
                std::process::exit(1);
            });
        tracing::info!(addr = %addr, "Listening");

        let shutdown_timeout = config.server.shutdown_timeout_secs;
        let server_shutdown = tokio_util::sync::CancellationToken::new();
        let server_shutdown_wait = server_shutdown.clone();
        let server_task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    server_shutdown_wait.cancelled().await;
                })
                .await
        });

        let shutdown_state = state.clone();
        let shutdown_signal_token = server_shutdown.clone();
        #[cfg(feature = "ws")]
        let websocket_shutdown = state.shutdown.clone();

        let shutdown_task = tokio::spawn(async move {
            shutdown_signal().await;
            shutdown_state.begin_shutdown();

            #[cfg(feature = "ws")]
            websocket_shutdown.cancel();

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

            run_shutdown_hooks(&shutdown_hooks).await;
            shutdown_signal_token.cancel();
        });

        if let Err(error) = run_startup_hooks(&startup_hooks, state.clone()).await {
            tracing::error!(error = %error, "startup hook failed");
            server_shutdown.cancel();
            server_task.abort();
            std::process::exit(1);
        }

        if !state.probes().is_shutting_down() {
            if !tasks.is_empty() {
                start_task_scheduler(tasks, &state, server_shutdown.clone());
            }
            state.probes().mark_startup_complete();
        }

        let server_result = server_task.await.unwrap_or_else(|e| {
            tracing::error!("Server task join error: {e}");
            std::process::exit(1);
        });
        shutdown_task.abort();
        server_result.unwrap_or_else(|e| {
            tracing::error!("Server error: {e}");
            std::process::exit(1);
        });

        tracing::info!("Server shut down cleanly");
    }

    /// Render all registered static routes to `dist/` and exit.
    ///
    /// Triggered when `AUTUMN_BUILD_STATIC=1` is set (by `autumn build`).
    /// Builds the Axum router, renders each static route through it, and
    /// writes HTML + manifest to the `dist/` directory.
    async fn run_build_mode(self) {
        let Self {
            routes,
            tasks: _,
            static_metas,
            exception_filters: _,
            scoped_groups: _,
            merge_routers: _,
            nest_routers: _,
            startup_hooks: _,
            shutdown_hooks: _,
            extensions: _,
            error_page_renderer: _,
            #[cfg(feature = "db")]
                migrations: _,
        } = self;

        let all_routes = routes;

        // Load config (same as normal startup)
        let (config, _telemetry_guard) = load_config_and_telemetry();

        if static_metas.is_empty() {
            eprintln!("No static routes registered. Nothing to build.");
            eprintln!("Hint: use .static_routes(static_routes![...]) on your AppBuilder.");
            std::process::exit(1);
        }

        // Build state (with DB if configured)
        #[cfg(feature = "db")]
        let pool = setup_database(&config, vec![]).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });

        let mut state = build_state(
            &config,
            #[cfg(feature = "db")]
            pool,
        );
        // run_build_mode used ProbeState::default(), which does not start as pending
        state.probes = crate::probe::ProbeState::default();

        // Build the full router (same as production)
        let router =
            crate::router::try_build_router(all_routes, &config, state).unwrap_or_else(|error| {
                eprintln!("Failed to build router: {error}");
                std::process::exit(1);
            });

        let env = crate::config::OsEnv;
        let dist_dir = project_dir("dist", &env);

        eprintln!("Building {} static route(s)...", static_metas.len());

        match crate::static_gen::render_static_routes(router, &static_metas, &dist_dir).await {
            Ok(()) => {
                eprintln!(
                    "\n  \u{2713} Static build complete \u{2192} {}",
                    dist_dir.display()
                );
            }
            Err(e) => {
                eprintln!("\n  \u{2717} Static build failed: {e}");
                std::process::exit(1);
            }
        }
    }
}

pub(crate) fn is_static_build_mode() -> bool {
    std::env::var("AUTUMN_BUILD_STATIC").as_deref() == Ok("1")
}

/// Start scheduled tasks in background Tokio tasks.
///
/// Each task runs in its own spawned task with error logging.
/// Uses `tokio::time` for fixed-delay scheduling and `tokio-cron-scheduler`
/// for cron-based scheduling. The `shutdown` token is used to stop the cron
/// scheduler gracefully when the server receives a termination signal.
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cognitive_complexity)]
fn start_task_scheduler(
    tasks: Vec<crate::task::TaskInfo>,
    state: &AppState,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tracing::info!(count = tasks.len(), "Starting scheduled tasks");
    for task_info in &tasks {
        let schedule_desc = match &task_info.schedule {
            crate::task::Schedule::FixedDelay(d) => format!("every {}s", d.as_secs()),
            crate::task::Schedule::Cron { expression, .. } => format!("cron {expression}"),
        };
        tracing::info!(name = %task_info.name, schedule = %schedule_desc, "Registered task");
    }

    let mut cron_tasks: Vec<(String, String, Option<String>, crate::task::TaskHandler)> =
        Vec::new();

    for task_info in tasks {
        let state = state.clone();
        let name = task_info.name.clone();
        let handler = task_info.handler;

        match task_info.schedule {
            crate::task::Schedule::FixedDelay(delay) => {
                // Register with the task registry for /actuator/tasks
                let schedule_desc = format!("every {}s", delay.as_secs());
                state.task_registry.register(&name, &schedule_desc);

                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(delay).await;
                        tracing::debug!(task = %name, "Running scheduled task");
                        state.task_registry.record_start(&name);
                        #[cfg(feature = "ws")]
                        {
                            let msg = serde_json::json!({
                                "event": "started",
                                "task": name,
                                "timestamp": chrono::Utc::now().to_rfc3339()
                            });
                            let _ = state.channels().sender("sys:tasks").send(msg.to_string());
                        }

                        let start = std::time::Instant::now();
                        match (handler)(state.clone()).await {
                            Ok(()) => {
                                let duration_ms =
                                    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                                state.task_registry.record_success(&name, duration_ms);
                                tracing::debug!(task = %name, "Task completed");

                                #[cfg(feature = "ws")]
                                {
                                    let msg = serde_json::json!({
                                        "event": "success",
                                        "task": name,
                                        "duration_ms": duration_ms,
                                        "timestamp": chrono::Utc::now().to_rfc3339()
                                    });
                                    let _ =
                                        state.channels().sender("sys:tasks").send(msg.to_string());
                                }
                            }
                            Err(e) => {
                                let duration_ms =
                                    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                                let error_str = e.to_string();
                                state
                                    .task_registry
                                    .record_failure(&name, duration_ms, &error_str);
                                tracing::warn!(task = %name, error = %e, "Task failed");

                                #[cfg(feature = "ws")]
                                {
                                    let msg = serde_json::json!({
                                        "event": "failure",
                                        "task": name,
                                        "duration_ms": duration_ms,
                                        "error": error_str,
                                        "timestamp": chrono::Utc::now().to_rfc3339()
                                    });
                                    let _ =
                                        state.channels().sender("sys:tasks").send(msg.to_string());
                                }
                            }
                        }
                    }
                });
            }
            crate::task::Schedule::Cron {
                expression,
                timezone,
            } => {
                let schedule_desc = format!("cron {expression}");
                state.task_registry.register(&name, &schedule_desc);
                cron_tasks.push((name, expression, timezone, handler));
            }
        }
    }

    if !cron_tasks.is_empty() {
        let state = state.clone();
        tokio::spawn(async move {
            run_cron_scheduler(cron_tasks, state, shutdown).await;
        });
    }
}

#[allow(unused_variables)]
fn send_ws_sys_task_msg(
    state: &AppState,
    event: &str,
    name: &str,
    extra: Option<(&str, serde_json::Value)>,
) {
    #[cfg(feature = "ws")]
    {
        let mut map = serde_json::Map::new();
        map.insert(
            "event".to_string(),
            serde_json::Value::String(event.to_string()),
        );
        map.insert(
            "task".to_string(),
            serde_json::Value::String(name.to_string()),
        );
        map.insert(
            "timestamp".to_string(),
            serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
        );
        if let Some((k, v)) = extra {
            map.insert(k.to_string(), v);
        }
        let msg = serde_json::Value::Object(map);
        let _ = state.channels().sender("sys:tasks").send(msg.to_string());
    }
}

async fn execute_cron_task_result(
    state: &AppState,
    handler: crate::task::TaskHandler,
    start: std::time::Instant,
) -> Result<u64, (u64, String)> {
    match (handler)(state.clone()).await {
        Ok(()) => {
            let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            Ok(duration_ms)
        }
        Err(e) => {
            let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            Err((duration_ms, e.to_string()))
        }
    }
}

/// Handle the execution of a single cron task.
async fn execute_cron_task(name: String, state: AppState, handler: crate::task::TaskHandler) {
    tracing::debug!(task = %name, "Running cron task");
    state.task_registry.record_start(&name);

    send_ws_sys_task_msg(&state, "started", &name, None);

    let start = std::time::Instant::now();
    match execute_cron_task_result(&state, handler, start).await {
        Ok(duration_ms) => {
            state.task_registry.record_success(&name, duration_ms);
            tracing::debug!(task = %name, "Cron task completed");
            send_ws_sys_task_msg(
                &state,
                "success",
                &name,
                Some(("duration_ms", serde_json::json!(duration_ms))),
            );
        }
        Err((duration_ms, error_str)) => {
            state
                .task_registry
                .record_failure(&name, duration_ms, &error_str);
            tracing::warn!(task = %name, error = %error_str, "Cron task failed");
            send_ws_sys_task_msg(
                &state,
                "failure",
                &name,
                Some(("error", serde_json::json!(error_str))),
            );
        }
    }
}

async fn register_cron_task(
    sched: &tokio_cron_scheduler::JobScheduler,
    name: String,
    expression: String,
    timezone: Option<String>,
    handler: crate::task::TaskHandler,
    state: AppState,
) {
    let state_clone = state.clone();
    let name_clone = name.clone();

    let job_result = build_cron_job(&expression, timezone.as_deref(), move |_uuid, _lock| {
        let state = state_clone.clone();
        let name = name_clone.clone();
        Box::pin(async move {
            execute_cron_task(name, state, handler).await;
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    match job_result {
        Ok(job) => {
            if let Err(e) = sched.add(job).await {
                tracing::error!(task = %name, error = %e, "Failed to add cron task to scheduler");
            }
        }
        Err(e) => {
            tracing::error!(task = %name, error = %e, "Failed to create cron job");
        }
    }
}

async fn setup_cron_scheduler(
    tasks: Vec<(String, String, Option<String>, crate::task::TaskHandler)>,
    state: AppState,
) -> Option<tokio_cron_scheduler::JobScheduler> {
    use tokio_cron_scheduler::JobScheduler;

    let sched = match JobScheduler::new().await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create cron job scheduler");
            return None;
        }
    };

    for (name, expression, timezone, handler) in tasks {
        register_cron_task(&sched, name, expression, timezone, handler, state.clone()).await;
    }

    if let Err(e) = sched.start().await {
        tracing::error!(error = %e, "Failed to start cron scheduler");
        return None;
    }

    Some(sched)
}

/// Run the `tokio-cron-scheduler` for all cron tasks, shutting down when the
/// `shutdown` token is cancelled.
#[allow(clippy::cognitive_complexity)]
async fn run_cron_scheduler(
    tasks: Vec<(String, String, Option<String>, crate::task::TaskHandler)>,
    state: AppState,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let Some(mut sched) = setup_cron_scheduler(tasks, state).await else {
        return;
    };

    tracing::info!("Cron scheduler started");
    shutdown.cancelled().await;
    tracing::info!("Shutting down cron scheduler");

    if let Err(e) = sched.shutdown().await {
        tracing::error!(error = %e, "Failed to shut down cron scheduler");
    }
}

/// Build a cron [`Job`](tokio_cron_scheduler::Job) for the given expression and optional
/// IANA timezone string.
///
/// If `timezone` is `None` or cannot be parsed, UTC is used.
fn build_cron_job<F>(
    expression: &str,
    timezone: Option<&str>,
    run: F,
) -> Result<tokio_cron_scheduler::Job, tokio_cron_scheduler::JobSchedulerError>
where
    F: 'static
        + FnMut(
            uuid::Uuid,
            tokio_cron_scheduler::JobScheduler,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
{
    use tokio_cron_scheduler::Job;

    if let Some(tz_str) = timezone {
        match tz_str.parse::<chrono_tz::Tz>() {
            Ok(tz) => return Job::new_async_tz(expression, tz, run),
            Err(_) => {
                tracing::warn!(
                    timezone = %tz_str,
                    "Unrecognized timezone; falling back to UTC"
                );
            }
        }
    }
    Job::new_async(expression, run)
}

async fn run_startup_hooks(hooks: &[StartupHook], state: AppState) -> crate::AutumnResult<()> {
    for hook in hooks {
        hook(state.clone()).await?;
    }
    Ok(())
}

async fn run_shutdown_hooks(hooks: &[ShutdownHook]) {
    for hook in hooks.iter().rev() {
        hook().await;
    }
}

/// Log a structured startup transparency report.
///
/// Activated by setting `AUTUMN_SHOW_CONFIG=1` (or `autumn dev --show-config`).
/// Prints all registered routes, scheduled tasks, active middleware, and
/// resolved configuration to the `INFO` log so developers can see exactly
/// what the macros and conventions configured.
#[allow(clippy::cognitive_complexity)]
fn log_startup_transparency(
    routes: &[Route],
    tasks: &[crate::task::TaskInfo],
    scoped_groups: &[ScopedGroup],
    config: &AutumnConfig,
) {
    tracing::info!(
        "Registered routes:{}",
        format_route_lines(routes, scoped_groups, config)
    );

    if let Some(task_lines) = format_task_lines(tasks) {
        tracing::info!("Scheduled tasks:{task_lines}");
    }

    tracing::info!("Active middleware: {}", format_middleware_list(config));

    tracing::info!("Configuration:{}", format_config_summary(config));
}

fn load_config_and_telemetry() -> (AutumnConfig, crate::telemetry::TelemetryGuard) {
    // 1. Load configuration (profile-aware)
    let config = AutumnConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load configuration: {e}");
        std::process::exit(1);
    });

    // 2. Initialize logging/telemetry immediately after config.
    let telemetry_guard = crate::logging::init_with_telemetry(
        &config.log,
        &config.telemetry,
        config.profile.as_deref(),
    )
    .unwrap_or_else(|error| {
        eprintln!("Failed to initialize telemetry: {error}");
        std::process::exit(1);
    });

    (config, telemetry_guard)
}

#[cfg(feature = "db")]
fn setup_database(
    config: &AutumnConfig,
    migrations: Vec<crate::migrate::EmbeddedMigrations>,
) -> Result<
    Option<diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>,
    String,
> {
    let pool = crate::db::create_pool(&config.database)
        .map_err(|e| format!("Failed to create database pool: {e}"))?;

    if let Some(url) = &config.database.url {
        for mig in migrations {
            crate::migrate::auto_migrate(url, config.profile.as_deref(), mig);
        }
    }

    Ok(pool)
}

fn build_state(
    config: &AutumnConfig,
    #[cfg(feature = "db")] pool: Option<
        diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    >,
) -> AppState {
    AppState {
        extensions: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        #[cfg(feature = "db")]
        pool,
        profile: config.profile.clone(),
        started_at: std::time::Instant::now(),
        health_detailed: config.health.detailed,
        probes: crate::probe::ProbeState::pending_startup(),
        metrics: crate::middleware::MetricsCollector::new(),
        log_levels: crate::actuator::LogLevels::new(&config.log.level),
        task_registry: crate::actuator::TaskRegistry::new(),
        config_props: crate::actuator::ConfigProperties::from_config(config),
        #[cfg(feature = "ws")]
        channels: crate::channels::Channels::new(32),
        #[cfg(feature = "ws")]
        shutdown: tokio_util::sync::CancellationToken::new(),
    }
}

/// Build the route listing string for the transparency log.
fn format_route_lines(
    routes: &[Route],
    scoped_groups: &[ScopedGroup],
    config: &AutumnConfig,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for route in routes {
        let _ = write!(
            out,
            "\n    {} {:<8} -> {}",
            route.path, route.method, route.name
        );
    }
    for group in scoped_groups {
        for route in &group.routes {
            let _ = write!(
                out,
                "\n    {}{} {:<8} -> {} (scoped)",
                group.prefix, route.path, route.method, route.name
            );
        }
    }
    let mut probe_paths = std::collections::HashSet::new();
    for (path, name) in [
        (config.health.live_path.as_str(), "live"),
        (config.health.ready_path.as_str(), "ready"),
        (config.health.startup_path.as_str(), "startup"),
        (config.health.path.as_str(), "health"),
    ] {
        if probe_paths.insert(path) {
            let _ = write!(out, "\n    {} {:<8} -> {}", path, "GET", name);
        }
    }
    let _ = write!(
        out,
        "\n    {} {:<8} -> actuator",
        crate::actuator::actuator_route_glob(&config.actuator.prefix),
        "GET"
    );
    #[cfg(feature = "htmx")]
    out.push_str("\n    /static/js/htmx.min.js GET -> htmx");
    out
}

/// Build the scheduled task listing string. Returns `None` if there are no tasks.
fn format_task_lines(tasks: &[crate::task::TaskInfo]) -> Option<String> {
    use std::fmt::Write as _;

    if tasks.is_empty() {
        return None;
    }

    let mut out = String::new();
    for task in tasks {
        let schedule = match &task.schedule {
            crate::task::Schedule::FixedDelay(d) => format!("every {}s", d.as_secs()),
            crate::task::Schedule::Cron { expression, .. } => format!("cron \"{expression}\""),
        };
        let _ = write!(out, "\n    {} ({schedule})", task.name);
    }
    Some(out)
}

/// Build the active middleware listing string.
fn format_middleware_list(config: &AutumnConfig) -> String {
    let mut items = vec![
        "RequestId",
        "SecurityHeaders",
        "Session (in-memory)",
        "ErrorPages",
    ];
    if !config.cors.allowed_origins.is_empty() {
        items.push("CORS");
    }
    if config.security.csrf.enabled {
        items.push("CSRF");
    }
    items.push("Metrics");
    items.join(", ")
}

/// Mask a database URL password for safe logging.
fn mask_database_url(url: &str, pool_size: usize) -> String {
    if let Ok(mut parsed_url) = url::Url::parse(url) {
        if parsed_url.password().is_some() {
            let _ = parsed_url.set_password(Some("****"));
            return format!("{parsed_url} (pool_size={pool_size})");
        }
    }
    format!("{url} (pool_size={pool_size})")
}

/// Build the configuration summary string.
fn format_config_summary(config: &AutumnConfig) -> String {
    let profile = config.profile.as_deref().unwrap_or("none");
    let db_status = config.database.url.as_deref().map_or_else(
        || "not configured".to_owned(),
        |url| mask_database_url(url, config.database.pool_size),
    );
    let telemetry_status = if config.telemetry.enabled {
        let endpoint = config
            .telemetry
            .otlp_endpoint
            .as_deref()
            .unwrap_or("<missing endpoint>");
        format!("{:?} -> {endpoint}", config.telemetry.protocol)
    } else {
        "disabled".to_owned()
    };
    format!(
        "\
        \n    profile:    {profile}\
        \n    server:     {}:{}\
        \n    database:   {db_status}\
        \n    log_level:  {}\
        \n    log_format: {:?}\
        \n    telemetry:  {telemetry_status}\
        \n    health:     {} (detailed={})\
        \n    actuator:   sensitive={}\
        \n    shutdown:   {}s",
        config.server.host,
        config.server.port,
        config.log.level,
        config.log.format,
        config.health.path,
        config.health.detailed,
        config.actuator.sensitive,
        config.server.shutdown_timeout_secs,
    )
}

/// Resolve a project-relative subdirectory (e.g. `"dist"` or `"static"`)
/// against `AUTUMN_MANIFEST_DIR` if set, otherwise use it as-is.
pub(crate) fn project_dir(subdir: &str, env: &dyn crate::config::Env) -> std::path::PathBuf {
    env.var("AUTUMN_MANIFEST_DIR").map_or_else(
        |_| std::path::PathBuf::from(subdir),
        |d| std::path::PathBuf::from(d).join(subdir),
    )
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
    pub fn test_router(routes: Vec<Route>) -> axum::Router {
        let config = AutumnConfig::default();
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        crate::router::build_router(routes, &config, state)
    }

    /// Helper to create a simple GET route for testing.
    pub fn test_get_route(path: &'static str, name: &'static str) -> Route {
        Route {
            method: http::Method::GET,
            path,
            handler: axum::routing::get(|| async { "ok" }),
            name,
        }
    }

    #[test]
    fn app_builder_routes_adds_routes() {
        let builder = app();
        assert_eq!(builder.routes.len(), 0);

        let builder = builder.routes(vec![test_get_route("/1", "route1")]);
        assert_eq!(builder.routes.len(), 1);

        let builder = builder.routes(vec![
            test_get_route("/2", "route2"),
            test_get_route("/3", "route3"),
        ]);
        assert_eq!(builder.routes.len(), 3);

        assert_eq!(builder.routes[0].path, "/1");
        assert_eq!(builder.routes[1].path, "/2");
        assert_eq!(builder.routes[2].path, "/3");
    }

    #[test]
    fn app_builder_extensions_store_and_update_typed_values() {
        let builder = app()
            .with_extension::<String>("haunted".into())
            .update_extension::<String, _, _>(String::new, |value| value.push_str(" harvest"));

        let value = builder
            .extension::<String>()
            .expect("string extension should be present");
        assert_eq!(value, "haunted harvest");
    }

    #[tokio::test]
    async fn startup_and_shutdown_hooks_run_in_expected_order() {
        let events = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let startup_events = Arc::clone(&events);
        let shutdown_a = Arc::clone(&events);
        let shutdown_b = Arc::clone(&events);
        let builder = app()
            .on_startup(move |_state| {
                let startup_events = Arc::clone(&startup_events);
                async move {
                    startup_events
                        .lock()
                        .expect("events lock poisoned")
                        .push("start");
                    Ok(())
                }
            })
            .on_shutdown(move || {
                let shutdown_a = Arc::clone(&shutdown_a);
                async move {
                    shutdown_a
                        .lock()
                        .expect("events lock poisoned")
                        .push("stop-a");
                }
            })
            .on_shutdown(move || {
                let shutdown_b = Arc::clone(&shutdown_b);
                async move {
                    shutdown_b
                        .lock()
                        .expect("events lock poisoned")
                        .push("stop-b");
                }
            });

        run_startup_hooks(&builder.startup_hooks, AppState::for_test())
            .await
            .expect("startup hooks should succeed");
        run_shutdown_hooks(&builder.shutdown_hooks).await;

        let recorded_events = events.lock().expect("events lock poisoned").clone();
        assert_eq!(recorded_events, vec!["start", "stop-b", "stop-a"]);
    }

    #[tokio::test]
    async fn startup_hook_errors_propagate() {
        let builder = app().on_startup(|_state| async {
            Err(crate::AutumnError::service_unavailable_msg(
                "startup ritual failed",
            ))
        });

        let error = run_startup_hooks(&builder.startup_hooks, AppState::for_test())
            .await
            .expect_err("startup hook should fail");
        assert!(error.to_string().contains("startup ritual failed"));
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
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        let router =
            crate::router::build_router(vec![test_get_route("/dummy", "dummy")], &config, state);

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
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        let router = crate::router::build_router(post_routes, &config, state);

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

    #[tokio::test]
    async fn build_router_merges_methods_on_same_path() {
        let route_list = vec![
            Route {
                method: http::Method::GET,
                path: "/admin",
                handler: axum::routing::get(|| async { "list" }),
                name: "admin_list",
            },
            Route {
                method: http::Method::POST,
                path: "/admin",
                handler: axum::routing::post(|| async { "created" }),
                name: "create",
            },
        ];
        let config = AutumnConfig::default();
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        let router = crate::router::build_router(route_list, &config, state);

        // GET /admin should return "list"
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"list");

        // POST /admin should return "created" (not 405!)
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"created");
    }

    #[cfg(feature = "htmx")]
    #[tokio::test]
    async fn htmx_handler_returns_javascript_with_correct_headers() {
        let app = axum::Router::new().route(
            "/static/js/htmx.min.js",
            axum::routing::get(crate::router::htmx_handler),
        );

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

    #[tokio::test]
    async fn build_router_serves_static_files_for_unmatched_paths() {
        use std::collections::HashMap;

        // Create a temp dist/ with a static page
        let tmp = tempfile::tempdir().expect("tempdir");
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(dist.join("docs")).expect("mkdir");
        std::fs::write(dist.join("docs/index.html"), "<h1>Static Docs</h1>").expect("write");

        let manifest = crate::static_gen::StaticManifest {
            generated_at: "2026-03-27T00:00:00Z".to_owned(),
            autumn_version: "0.2.0".to_owned(),
            routes: HashMap::from([(
                "/docs".to_owned(),
                crate::static_gen::ManifestEntry {
                    file: "docs/index.html".to_owned(),
                    revalidate: None,
                },
            )]),
        };
        let json = serde_json::to_string(&manifest).expect("serialize");
        std::fs::write(dist.join("manifest.json"), json).expect("write manifest");

        // No dynamic route for /docs — only a static file.
        let config = AutumnConfig::default();
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        let router = crate::router::build_router_with_static(
            vec![test_get_route("/other", "other_page")],
            &config,
            state,
            Some(dist.as_path()),
        );

        // GET /docs/ should serve the pre-built HTML via static-first
        // middleware (manifest lookup with trailing-slash normalization).
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/docs/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "<h1>Static Docs</h1>");
    }

    #[tokio::test]
    async fn build_mode_static_rendering_bypasses_startup_barrier() {
        temp_env::async_with_vars([("AUTUMN_BUILD_STATIC", Some("1"))], async {
            let config = AutumnConfig::default();
            let state = AppState::for_test().with_startup_complete(false);
            let router = crate::router::build_router(
                vec![Route {
                    method: http::Method::GET,
                    path: "/about",
                    handler: axum::routing::get(|| async { "About Page Content" }),
                    name: "about",
                }],
                &config,
                state,
            );
            let tmp = tempfile::tempdir().unwrap();
            let dist = tmp.path().join("dist");

            let result = crate::static_gen::render_static_routes(
                router,
                &[crate::static_gen::StaticRouteMeta {
                    path: "/about",
                    name: "about",
                    revalidate: None,
                    params_fn: None,
                }],
                &dist,
            )
            .await;

            assert!(result.is_ok(), "build failed: {:?}", result.err());
            let html = std::fs::read_to_string(dist.join("about/index.html")).unwrap();
            assert_eq!(html, "About Page Content");
        })
        .await;
    }

    #[tokio::test]
    async fn build_router_injects_live_reload_script_when_enabled() {
        let reload_file = tempfile::NamedTempFile::new().expect("reload state file");
        std::fs::write(reload_file.path(), r#"{"version":0,"kind":"full"}"#).expect("write");
        temp_env::async_with_vars(
            [
                ("AUTUMN_DEV_RELOAD", Some("1")),
                (
                    "AUTUMN_DEV_RELOAD_STATE",
                    Some(reload_file.path().to_str().expect("utf-8 path")),
                ),
            ],
            async {
                let router = test_router(vec![Route {
                    method: http::Method::GET,
                    path: "/page",
                    handler: axum::routing::get(|| async {
                        axum::response::Html("<html><body><main>ok</main></body></html>")
                    }),
                    name: "page",
                }]);

                let response = router
                    .oneshot(Request::builder().uri("/page").body(Body::empty()).unwrap())
                    .await
                    .unwrap();

                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let html = std::str::from_utf8(&body).expect("utf-8");
                assert!(html.contains("/__autumn/live-reload"));
            },
        )
        .await;
    }

    #[tokio::test]
    async fn build_router_mounts_dev_reload_endpoint_when_enabled() {
        let reload_file = tempfile::NamedTempFile::new().expect("reload state file");
        std::fs::write(reload_file.path(), r#"{"version":7,"kind":"css"}"#).expect("write");
        temp_env::async_with_vars(
            [
                ("AUTUMN_DEV_RELOAD", Some("1")),
                (
                    "AUTUMN_DEV_RELOAD_STATE",
                    Some(reload_file.path().to_str().expect("utf-8 path")),
                ),
            ],
            async {
                let router = test_router(vec![test_get_route("/dummy", "dummy")]);

                let response = router
                    .oneshot(
                        Request::builder()
                            .uri("/__autumn/live-reload")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    response.headers().get("cache-control").unwrap(),
                    "no-store, no-cache, must-revalidate"
                );
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                assert_eq!(&body[..], br#"{"version":7,"kind":"css"}"#);
            },
        )
        .await;
    }

    #[tokio::test]
    async fn build_router_disables_cache_for_static_assets_in_dev_reload_mode() {
        let project = tempfile::tempdir().expect("project dir");
        let static_dir = project.path().join("static");
        std::fs::create_dir_all(&static_dir).expect("mkdir");
        std::fs::write(static_dir.join("demo.txt"), "hello").expect("write static file");
        let reload_file = tempfile::NamedTempFile::new().expect("reload state file");
        std::fs::write(reload_file.path(), r#"{"version":0,"kind":"full"}"#).expect("write");
        temp_env::async_with_vars(
            [
                (
                    "AUTUMN_MANIFEST_DIR",
                    Some(project.path().to_str().expect("utf-8 path")),
                ),
                ("AUTUMN_DEV_RELOAD", Some("1")),
                (
                    "AUTUMN_DEV_RELOAD_STATE",
                    Some(reload_file.path().to_str().expect("utf-8 path")),
                ),
            ],
            async {
                let router = test_router(vec![test_get_route("/dummy", "dummy")]);

                let response = router
                    .oneshot(
                        Request::builder()
                            .uri("/static/demo.txt")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    response.headers().get("cache-control").unwrap(),
                    "no-store, no-cache, must-revalidate"
                );
            },
        )
        .await;
    }

    #[test]
    fn app_builder_accepts_static_routes() {
        use crate::static_gen::StaticRouteMeta;
        let metas = vec![StaticRouteMeta {
            path: "/about",
            name: "about",
            revalidate: None,
            params_fn: None,
        }];
        let builder = app().static_routes(metas);
        assert_eq!(builder.static_metas.len(), 1);
    }

    #[test]
    fn project_dir_defaults_to_subdir() {
        // When AUTUMN_MANIFEST_DIR is not set, project_dir returns the
        // subdir name as-is (relative to cwd).
        let env = crate::config::MockEnv::new();
        let dir = super::project_dir("dist", &env);
        assert_eq!(dir, std::path::PathBuf::from("dist"));
    }

    /// Helper to build a test router with custom config.
    pub fn test_router_with_config(routes: Vec<Route>, config: &AutumnConfig) -> axum::Router {
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        crate::router::build_router(routes, config, state)
    }

    #[tokio::test]
    async fn cors_wildcard_allows_any_origin() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["*".to_owned()];
        let router = test_router_with_config(vec![test_get_route("/test", "test")], &config);

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
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "*"
        );
    }

    #[tokio::test]
    async fn cors_specific_origin_reflected() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["https://example.com".to_owned()];
        let router = test_router_with_config(vec![test_get_route("/test", "test")], &config);

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
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "https://example.com"
        );
    }

    #[tokio::test]
    async fn cors_disabled_when_no_origins() {
        let config = AutumnConfig::default();
        assert!(config.cors.allowed_origins.is_empty());
        let router = test_router_with_config(vec![test_get_route("/test", "test")], &config);

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
                .is_none()
        );
    }

    #[tokio::test]
    async fn cors_preflight_returns_204() {
        let mut config = AutumnConfig::default();
        config.cors.allowed_origins = vec!["https://example.com".to_owned()];
        let router = test_router_with_config(vec![test_get_route("/test", "test")], &config);

        let response = router
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/test")
                    .header("Origin", "https://example.com")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .contains_key("access-control-allow-methods")
        );
    }

    #[tokio::test]
    async fn build_router_with_static_skips_without_manifest() {
        // When dist/ exists but has no manifest.json, fall back to
        // the app router without the static layer.
        let tmp = tempfile::tempdir().expect("tempdir");
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).expect("mkdir");
        // No manifest.json — just an empty dist/

        let config = AutumnConfig::default();
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        let router = crate::router::build_router_with_static(
            vec![test_get_route("/test", "test")],
            &config,
            state,
            Some(dist.as_path()),
        );

        let response = router
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn build_router_with_static_none_dist() {
        // When dist_dir is None, return the app router directly.
        let config = AutumnConfig::default();
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        let router = crate::router::build_router_with_static(
            vec![test_get_route("/test", "test")],
            &config,
            state,
            None,
        );

        let response = router
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // ── Startup transparency helper tests ─────────────────────────

    #[test]
    fn format_route_lines_lists_user_routes() {
        let routes = vec![
            test_get_route("/", "index"),
            test_get_route("/users/{id}", "get_user"),
        ];
        let config = AutumnConfig::default();
        let output = format_route_lines(&routes, &[], &config);
        assert!(output.contains("-> index"));
        assert!(output.contains("/ GET"));
        assert!(output.contains("/users/{id}"));
        assert!(output.contains("-> get_user"));
    }

    #[test]
    fn config_runtime_drift_format_route_lines_uses_actuator_prefix() {
        let mut config = AutumnConfig::default();
        config.actuator.prefix = "/ops".to_owned();
        let output = format_route_lines(&[], &[], &config);
        assert!(output.contains("-> health"));
        assert!(output.contains("/ops/*"));
    }

    #[test]
    fn format_task_lines_none_when_empty() {
        assert!(format_task_lines(&[]).is_none());
    }

    #[test]
    fn format_task_lines_fixed_delay() {
        let tasks = vec![crate::task::TaskInfo {
            name: "cleanup".into(),
            schedule: crate::task::Schedule::FixedDelay(std::time::Duration::from_secs(300)),
            handler: |_| Box::pin(async { Ok(()) }),
        }];
        let output = format_task_lines(&tasks).unwrap();
        assert!(output.contains("cleanup (every 300s)"));
    }

    #[test]
    fn format_task_lines_cron() {
        let tasks = vec![crate::task::TaskInfo {
            name: "nightly".into(),
            schedule: crate::task::Schedule::Cron {
                expression: "0 0 * * *".into(),
                timezone: None,
            },
            handler: |_| Box::pin(async { Ok(()) }),
        }];
        let output = format_task_lines(&tasks).unwrap();
        assert!(output.contains("nightly (cron \"0 0 * * *\")"));
    }

    #[test]
    fn format_middleware_list_default() {
        let config = AutumnConfig::default();
        let output = format_middleware_list(&config);
        assert!(output.contains("RequestId"));
        assert!(output.contains("SecurityHeaders"));
        assert!(output.contains("Session (in-memory)"));
        assert!(output.contains("Metrics"));
        // CORS and CSRF should not be present with defaults
        assert!(!output.contains("CORS"));
        assert!(!output.contains("CSRF"));
    }

    #[test]
    fn format_middleware_list_with_cors_and_csrf() {
        let config = AutumnConfig {
            cors: crate::config::CorsConfig {
                allowed_origins: vec!["https://example.com".into()],
                ..crate::config::CorsConfig::default()
            },
            security: crate::security::config::SecurityConfig {
                csrf: crate::security::config::CsrfConfig {
                    enabled: true,
                    ..crate::security::config::CsrfConfig::default()
                },
                ..crate::security::config::SecurityConfig::default()
            },
            ..AutumnConfig::default()
        };
        let output = format_middleware_list(&config);
        assert!(output.contains("CORS"));
        assert!(output.contains("CSRF"));
    }

    #[test]
    fn mask_database_url_with_password() {
        let masked = mask_database_url("postgres://user:secret@localhost:5432/mydb", 10);
        assert!(masked.contains("****"));
        assert!(!masked.contains("secret"));
        assert!(masked.contains("postgres://user:****@localhost:5432/mydb"));
        assert!(masked.contains("pool_size=10"));
    }

    #[test]
    fn mask_database_url_without_password() {
        let masked = mask_database_url("postgres://localhost/mydb", 5);
        assert!(!masked.contains("****"));
        assert!(masked.contains("postgres://localhost/mydb"));
        assert!(masked.contains("pool_size=5"));
    }

    #[test]
    fn mask_database_url_edge_cases() {
        // Special chars in password
        // The url crate parses `p@ssw:rd!` where `@` creates problems if unencoded,
        // but url crate seems to treat `user:p` as auth and `@ssw:rd!` as host if it's poorly formed,
        // let's stick to valid URL formats for testing.

        // URL encoded characters
        let masked2 = mask_database_url("postgres://user:p%40ssw%3Ard%21@localhost:5432/mydb", 10);
        assert!(masked2.contains("****"));
        assert!(!masked2.contains("p%40ssw%3Ard%21"));
        assert!(masked2.contains("postgres://user:****@localhost:5432/mydb"));

        // No user, just password
        let masked3 = mask_database_url("postgres://:secret@localhost:5432/mydb", 10);
        assert!(masked3.contains("****"));
        assert!(!masked3.contains("secret"));
        assert!(masked3.contains("postgres://:****@localhost:5432/mydb"));
    }
    #[test]
    fn format_config_summary_defaults() {
        let config = AutumnConfig::default();
        let output = format_config_summary(&config);
        assert!(output.contains("profile:    none"));
        assert!(output.contains("server:     127.0.0.1:3000"));
        assert!(output.contains("database:   not configured"));
        assert!(output.contains("log_level:"));
        assert!(output.contains("telemetry:  disabled"));
        assert!(output.contains("health:     /health"));
    }

    #[test]
    fn format_config_summary_with_db() {
        let config = AutumnConfig {
            database: crate::config::DatabaseConfig {
                url: Some("postgres://user:pass@host/db".into()),
                pool_size: 20,
                ..crate::config::DatabaseConfig::default()
            },
            ..AutumnConfig::default()
        };
        let output = format_config_summary(&config);
        assert!(output.contains("user:****@host/db"));
        assert!(output.contains("pool_size=20"));
        assert!(!output.contains("pass"));
    }

    #[test]
    fn format_config_summary_with_profile() {
        let config = AutumnConfig {
            profile: Some("prod".into()),
            ..AutumnConfig::default()
        };
        let output = format_config_summary(&config);
        assert!(output.contains("profile:    prod"));
    }

    #[test]
    fn format_config_summary_with_telemetry() {
        let config = AutumnConfig {
            telemetry: crate::config::TelemetryConfig {
                enabled: true,
                service_name: "orders-api".into(),
                otlp_endpoint: Some("http://otel-collector:4317".into()),
                ..crate::config::TelemetryConfig::default()
            },
            ..AutumnConfig::default()
        };
        let output = format_config_summary(&config);
        assert!(output.contains("telemetry:  Grpc -> http://otel-collector:4317"));
    }

    #[test]
    fn log_startup_transparency_runs_without_panic() {
        // Exercises the tracing::info! calls inside log_startup_transparency.
        // No subscriber installed, so output is discarded -- we just verify
        // the function doesn't panic.
        let routes = vec![test_get_route("/", "index")];
        let tasks = vec![crate::task::TaskInfo {
            name: "cleanup".into(),
            schedule: crate::task::Schedule::FixedDelay(std::time::Duration::from_secs(60)),
            handler: |_| Box::pin(async { Ok(()) }),
        }];
        let config = AutumnConfig::default();
        log_startup_transparency(&routes, &tasks, &[], &config);
    }

    #[test]
    fn log_startup_transparency_no_tasks() {
        let routes = vec![test_get_route("/health", "check")];
        let config = AutumnConfig::default();
        log_startup_transparency(&routes, &[], &[], &config);
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn start_task_scheduler_broadcasts_events() {
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            channels: crate::channels::Channels::new(32),
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let mut rx = state.channels().subscribe("sys:tasks");

        let task = crate::task::TaskInfo {
            name: "test_broadcaster".into(),
            // 1ms delay so it fires immediately
            schedule: crate::task::Schedule::FixedDelay(std::time::Duration::from_millis(1)),
            handler: |_| Box::pin(async { Ok(()) }),
        };

        // Start scheduler in background so we don't block
        let state_clone = state.clone();
        tokio::spawn(async move {
            super::start_task_scheduler(
                vec![task],
                &state_clone,
                tokio_util::sync::CancellationToken::new(),
            );
        });

        // First message should be "started"
        let msg1 = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for start event")
            .expect("channel closed");
        let json1: serde_json::Value = serde_json::from_str(msg1.as_str()).unwrap();
        assert_eq!(json1["event"], "started");
        assert_eq!(json1["task"], "test_broadcaster");

        // Second message should be "success"
        let msg2 = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for success event")
            .expect("channel closed");
        let json2: serde_json::Value = serde_json::from_str(msg2.as_str()).unwrap();
        assert_eq!(json2["event"], "success");
        assert_eq!(json2["task"], "test_broadcaster");
        assert!(json2.get("duration_ms").is_some());
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn start_task_scheduler_broadcasts_failure_events() {
        let state = AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            channels: crate::channels::Channels::new(32),
            shutdown: tokio_util::sync::CancellationToken::new(),
        };

        let mut rx = state.channels().subscribe("sys:tasks");

        let task = crate::task::TaskInfo {
            name: "test_failing_task".into(),
            schedule: crate::task::Schedule::FixedDelay(std::time::Duration::from_millis(1)),
            handler: |_| {
                Box::pin(async { Err(crate::AutumnError::bad_request_msg("forced error")) })
            },
        };

        let state_clone = state.clone();
        tokio::spawn(async move {
            super::start_task_scheduler(
                vec![task],
                &state_clone,
                tokio_util::sync::CancellationToken::new(),
            );
        });

        // First message: started
        let _ = rx.recv().await.unwrap();

        // Second message: failure
        let msg2 = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for failure event")
            .expect("channel closed");
        let json2: serde_json::Value = serde_json::from_str(msg2.as_str()).unwrap();
        assert_eq!(json2["event"], "failure");
        assert_eq!(json2["task"], "test_failing_task");
        assert_eq!(json2["error"], "forced error");
    }

    #[tokio::test]
    async fn execute_cron_task_result_ok_returns_duration() {
        let state = AppState::for_test();
        let handler: crate::task::TaskHandler = |_| Box::pin(async { Ok(()) });
        let start = std::time::Instant::now();
        let result = super::execute_cron_task_result(&state, handler, start).await;
        assert!(result.is_ok(), "expected Ok from successful handler");
        // duration_ms should be a reasonable value (not MAX)
        assert!(result.unwrap() < u64::MAX);
    }

    #[tokio::test]
    async fn execute_cron_task_result_err_returns_duration_and_message() {
        let state = AppState::for_test();
        let handler: crate::task::TaskHandler =
            |_| Box::pin(async { Err(crate::AutumnError::bad_request_msg("test error")) });
        let start = std::time::Instant::now();
        let result = super::execute_cron_task_result(&state, handler, start).await;
        assert!(result.is_err(), "expected Err from failing handler");
        let (duration_ms, msg) = result.unwrap_err();
        assert!(duration_ms < u64::MAX);
        assert!(msg.contains("test error"));
    }
}
