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
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::Instrument as _;

use crate::config::{AutumnConfig, ConfigLoader};
#[cfg(feature = "db")]
use crate::db::DatabasePoolProvider;
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
        jobs: Vec::new(),
        static_metas: Vec::new(),
        exception_filters: Vec::new(),
        scoped_groups: Vec::new(),
        merge_routers: Vec::new(),
        nest_routers: Vec::new(),
        custom_layers: Vec::new(),
        startup_hooks: Vec::new(),
        shutdown_hooks: Vec::new(),
        extensions: HashMap::new(),
        registered_plugins: HashSet::new(),
        error_page_renderer: None,
        #[cfg(feature = "db")]
        migrations: Vec::new(),
        config_loader_factory: None,
        #[cfg(feature = "db")]
        pool_provider_factory: None,
        telemetry_provider: None,
        session_store: None,
        #[cfg(feature = "openapi")]
        openapi: None,
        audit_logger: None,
        policy_registrations: Vec::new(),
    }
}

type StartupHookFuture = Pin<Box<dyn Future<Output = crate::AutumnResult<()>> + Send>>;
type StartupHook = Box<dyn Fn(AppState) -> StartupHookFuture + Send + Sync>;
type ShutdownHookFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type ShutdownHook = Box<dyn Fn() -> ShutdownHookFuture + Send + Sync>;

// ── Tier-1 subsystem factories ────────────────────────────────
//
// `ConfigLoader` and `DatabasePoolProvider` use RPIT (`-> impl Future + Send`)
// in their trait methods, so `Box<dyn Trait>` is not dyn-compatible. We store
// boxed factory closures that capture the concrete impl at the call site and
// erase its future type via `Pin<Box<dyn Future>>`. `TelemetryProvider`'s
// `init` is sync, so it's stored as a normal `Box<dyn>`.
type ConfigLoaderFactory = Box<
    dyn FnOnce() -> Pin<
            Box<dyn Future<Output = Result<AutumnConfig, crate::config::ConfigError>> + Send>,
        > + Send,
>;
#[cfg(feature = "db")]
type PoolProviderFactory = Box<
    dyn FnOnce(
            crate::config::DatabaseConfig,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            Option<
                                diesel_async::pooled_connection::deadpool::Pool<
                                    diesel_async::AsyncPgConnection,
                                >,
                            >,
                            crate::db::PoolError,
                        >,
                    > + Send,
            >,
        > + Send,
>;

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
/// Closure that registers a policy or scope on the runtime
/// [`PolicyRegistry`](crate::authorization::PolicyRegistry).
type PolicyRegistration = Box<dyn FnOnce(&crate::authorization::PolicyRegistry) + Send>;

pub struct AppBuilder {
    routes: Vec<Route>,
    tasks: Vec<crate::task::TaskInfo>,
    jobs: Vec<crate::job::JobInfo>,
    pub(crate) static_metas: Vec<crate::static_gen::StaticRouteMeta>,
    exception_filters: Vec<Arc<dyn ExceptionFilter>>,
    scoped_groups: Vec<ScopedGroup>,
    merge_routers: Vec<axum::Router<AppState>>,
    nest_routers: Vec<(String, axum::Router<AppState>)>,
    /// Custom Tower layers registered via [`AppBuilder::layer`], applied
    /// inside `RequestIdLayer` on ingress so they observe the request ID.
    custom_layers: Vec<CustomLayerRegistration>,
    startup_hooks: Vec<StartupHook>,
    shutdown_hooks: Vec<ShutdownHook>,
    extensions: HashMap<TypeId, Box<dyn Any + Send>>,
    /// Plugin names that have already been applied, for duplicate detection.
    registered_plugins: HashSet<String>,
    /// Custom error page renderer (overrides built-in pages).
    error_page_renderer: Option<SharedRenderer>,
    /// Embedded Diesel migrations, registered via `.migrations()`.
    #[cfg(feature = "db")]
    migrations: Vec<migrate::EmbeddedMigrations>,
    /// Custom config loader (tier-1 subsystem replacement). When `None`, the
    /// default [`TomlEnvConfigLoader`](crate::config::TomlEnvConfigLoader) runs.
    config_loader_factory: Option<ConfigLoaderFactory>,
    /// Custom DB pool provider (tier-1 subsystem replacement). When `None`,
    /// the default [`DieselDeadpoolPoolProvider`](crate::db::DieselDeadpoolPoolProvider) runs.
    #[cfg(feature = "db")]
    pool_provider_factory: Option<PoolProviderFactory>,
    /// Custom telemetry provider (tier-1 subsystem replacement). When `None`,
    /// the default [`TracingOtlpTelemetryProvider`](crate::telemetry::TracingOtlpTelemetryProvider) runs.
    telemetry_provider: Option<Box<dyn crate::telemetry::TelemetryProvider>>,
    /// Custom session store (tier-1 subsystem replacement). When `Some`,
    /// `apply_session_layer` skips the config-driven `memory`/`redis` selection
    /// and uses this store directly.
    session_store: Option<Arc<dyn crate::session::BoxedSessionStore>>,
    /// `OpenAPI` generation configuration. When `Some`, the router mounts
    /// `/v3/api-docs` (serving `openapi.json`) and `/swagger-ui` (if the
    /// Swagger UI path is set). When `None`, no docs endpoints are mounted.
    ///
    /// Gated behind the `openapi` feature: apps that don't need a
    /// served `OpenAPI` document shouldn't pay for the spec types or the
    /// runtime collision-check machinery.
    #[cfg(feature = "openapi")]
    openapi: Option<crate::openapi::OpenApiConfig>,
    /// Shared audit logger used for append-only compliance events.
    audit_logger: Option<Arc<crate::audit::AuditLogger>>,
    /// Deferred [`Policy`](crate::authorization::Policy) and
    /// [`Scope`](crate::authorization::Scope) registrations applied
    /// to [`AppState::policy_registry`] just before the router is
    /// built. Stored as boxed closures so we can carry the
    /// generic type parameters across the builder boundary.
    policy_registrations: Vec<PolicyRegistration>,
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

/// A deferred router mutator that applies a user-registered
/// [`tower::Layer`] to the app-wide router.
///
/// Stored on [`AppBuilder`] by [`AppBuilder::layer`] and drained inside
/// `apply_middleware` where the final layer stack is assembled.
pub(crate) type CustomLayerApplier =
    Box<dyn FnOnce(axum::Router<AppState>) -> axum::Router<AppState> + Send>;

/// Metadata and deferred application closure for a user-registered layer.
pub(crate) struct CustomLayerRegistration {
    /// Concrete type for the registered layer.
    pub(crate) type_id: TypeId,
    /// Deferred router mutation that applies the layer.
    pub(crate) apply: CustomLayerApplier,
}

mod sealed {
    pub trait Sealed {}
}

/// Marker trait for types that can be registered with
/// [`AppBuilder::layer`] as an app-wide Tower middleware.
///
/// Any [`tower::Layer`] whose produced service is a compatible axum
/// service (i.e. `Service<Request, Response = Response, Error = Infallible>`,
/// plus the usual `Clone + Send + Sync + 'static` bounds and a `Send`
/// future) implements this trait automatically via a blanket impl.
///
/// The trait is **sealed**: it exists only to surface a clean
/// `IntoAppLayer is not implemented for YourType` error message when a
/// candidate layer fails to meet axum's service bounds, instead of a
/// 40-line associated-type wall. You cannot implement it manually, and
/// you should not need to — just bring your own `tower::Layer`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a usable Autumn app-wide Tower layer",
    label = "this type does not implement `tower::Layer<axum::routing::Route>` with the required service bounds",
    note = "`AppBuilder::layer(..)` requires:\n    L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,\n    L::Service: Service<axum::extract::Request, Response = axum::response::Response, Error = Infallible> + Clone + Send + Sync + 'static,\n    <L::Service as Service<axum::extract::Request>>::Future: Send + 'static\nSee docs/guide/middleware.md for common patterns and how to wrap raw-error layers (e.g. TimeoutLayer) with HandleErrorLayer."
)]
pub trait IntoAppLayer: sealed::Sealed + Send + Sync + 'static {
    /// Apply this layer to the given router. Not intended for direct use.
    #[doc(hidden)]
    fn apply_to(self, router: axum::Router<AppState>) -> axum::Router<AppState>;
}

impl<L> sealed::Sealed for L
where
    L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
    L::Service: tower::Service<
            axum::extract::Request,
            Response = axum::response::Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + Sync
        + 'static,
    <L::Service as tower::Service<axum::extract::Request>>::Future: Send + 'static,
{
}

impl<L> IntoAppLayer for L
where
    L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
    L::Service: tower::Service<
            axum::extract::Request,
            Response = axum::response::Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + Sync
        + 'static,
    <L::Service as tower::Service<axum::extract::Request>>::Future: Send + 'static,
{
    fn apply_to(self, router: axum::Router<AppState>) -> axum::Router<AppState> {
        router.layer(self)
    }
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

    /// Register ad-hoc background jobs with the application.
    #[must_use]
    pub fn jobs(mut self, jobs: Vec<crate::job::JobInfo>) -> Self {
        self.jobs.extend(jobs);
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

    /// Enable `OpenAPI` (Swagger) spec auto-generation.
    ///
    /// When called, the framework inspects every registered route's
    /// [`ApiDoc`](crate::openapi::ApiDoc) metadata — inferred at compile
    /// time from the route path, HTTP method, extractor types, and any
    /// [`#[api_doc(...)]`](crate::api_doc) overrides — and serves an
    /// `OpenAPI` 3.0 JSON document at `OpenApiConfig::openapi_json_path`
    /// (default `/v3/api-docs`). If
    /// `OpenApiConfig::swagger_ui_path` is set (default `/swagger-ui`),
    /// a Swagger UI HTML page is served there too.
    ///
    /// Routes marked `#[api_doc(hidden)]` are excluded.
    ///
    /// **Gated behind the `openapi` Cargo feature.** Add
    /// `features = ["openapi"]` to your `autumn-web` dependency to
    /// enable it; the default build excludes the runtime spec types
    /// and endpoints to keep the binary small.
    ///
    /// # Examples
    ///
    /// Zero-config:
    ///
    /// ```rust,ignore
    /// use autumn_web::prelude::*;
    /// use autumn_web::openapi::OpenApiConfig;
    ///
    /// # #[get("/hello")] async fn hello() -> &'static str { "hi" }
    /// # #[autumn_web::main]
    /// # async fn main() {
    /// autumn_web::app()
    ///     .routes(routes![hello])
    ///     .openapi(OpenApiConfig::new("My API", "1.0.0"))
    ///     .run()
    ///     .await;
    /// # }
    /// ```
    ///
    /// With custom paths:
    ///
    /// ```rust,ignore
    /// use autumn_web::openapi::OpenApiConfig;
    ///
    /// let config = OpenApiConfig::new("My API", "1.0.0")
    ///     .description("Full product API")
    ///     .openapi_json_path("/openapi.json")
    ///     .swagger_ui_path(Some("/docs".to_owned()));
    /// ```
    #[cfg(feature = "openapi")]
    #[must_use]
    pub fn openapi(mut self, config: crate::openapi::OpenApiConfig) -> Self {
        self.openapi = Some(config);
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

    /// Apply a custom [`tower::Layer`] to the entire application.
    ///
    /// This is the escape hatch for integrating any middleware from the
    /// Tower / Tower-HTTP ecosystem (timeouts, rate limiting, bespoke
    /// tracing, request signing, etc.) without forking the framework.
    ///
    /// The generic bound is [`IntoAppLayer`], a sealed trait with a blanket
    /// impl for every `tower::Layer` that meets axum's service requirements
    /// — in practice this means any standard Tower layer whose service
    /// produces `Infallible` errors. If your layer produces real errors
    /// (like `TimeoutLayer`'s `BoxError`), wrap it with
    /// [`axum::error_handling::HandleErrorLayer`] before passing it here.
    ///
    /// # Ordering
    ///
    /// User layers are applied **inside** Autumn's request-ID layer on the
    /// ingress path, which means your middleware always sees the generated
    /// `RequestId` in the request extensions. The full stack (outermost to
    /// innermost on ingress) is:
    ///
    /// `Metrics -> ExceptionFilter -> ErrorPageContext -> Session ->`
    /// `SecurityHeaders -> RequestId -> [user layers, registration order]`
    /// `-> CSRF -> CORS -> route handler`
    ///
    /// When `.layer()` is called multiple times, the **first** call becomes
    /// the outermost user layer on ingress (matching `tower::ServiceBuilder`
    /// semantics): the layer from the first `.layer(...)` call sees the
    /// request first on the way in and the response last on the way out.
    ///
    /// # Scope
    ///
    /// This layer applies **globally** to every route in the app, including
    /// routes added later by plugins, routes mounted via `.merge` / `.nest`,
    /// and the built-in `404` fallback. Use [`AppBuilder::scoped`] when you
    /// need middleware scoped to a group of routes.
    ///
    /// Shared state (pools, metrics registries, rate-limit stores, etc.)
    /// should be wrapped in `Arc` so the layer can satisfy the
    /// `Clone + Send + Sync + 'static` bounds without moving the state.
    ///
    /// See [the middleware guide](https://github.com/madmax983/autumn/blob/trunk/docs/guide/middleware.md)
    /// for ready-made recipes.
    ///
    /// # Examples
    ///
    /// Adding a Tower timeout layer in one line (Tower's `TimeoutLayer`
    /// returns `BoxError`, so it must be paired with `HandleErrorLayer` to
    /// satisfy axum's `Infallible` error requirement):
    ///
    /// ```rust,no_run
    /// use std::time::Duration;
    /// use autumn_web::prelude::*;
    /// use axum::{error_handling::HandleErrorLayer, http::StatusCode};
    /// use tower::{ServiceBuilder, timeout::TimeoutLayer};
    ///
    /// # #[get("/")] async fn index() -> &'static str { "ok" }
    /// # #[autumn_web::main]
    /// # async fn main() {
    /// autumn_web::app()
    ///     .routes(routes![index])
    ///     .layer(
    ///         ServiceBuilder::new()
    ///             .layer(HandleErrorLayer::new(|_| async {
    ///                 StatusCode::REQUEST_TIMEOUT
    ///             }))
    ///             .layer(TimeoutLayer::new(Duration::from_secs(5))),
    ///     )
    ///     .run()
    ///     .await;
    /// # }
    /// ```
    #[must_use]
    pub fn layer<L: IntoAppLayer>(mut self, layer: L) -> Self {
        self.custom_layers.push(CustomLayerRegistration {
            type_id: TypeId::of::<L>(),
            apply: Box::new(move |router| layer.apply_to(router)),
        });
        self
    }

    /// Returns `true` when a custom layer of type `L` has already been
    /// registered via [`AppBuilder::layer`].
    ///
    /// Intended for plugin pre-flight validation before the app is started.
    #[must_use]
    pub fn has_layer<L: 'static>(&self) -> bool {
        let layer_type = TypeId::of::<L>();
        self.custom_layers
            .iter()
            .any(|registered| registered.type_id == layer_type)
    }

    /// Returns the registered custom layer types in registration order.
    ///
    /// This includes only user-installed layers from
    /// [`AppBuilder::layer`], not framework-managed middleware.
    #[must_use]
    pub fn get_layer_types(&self) -> Vec<TypeId> {
        self.custom_layers
            .iter()
            .map(|registered| registered.type_id)
            .collect()
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
    /// Merged routes are added **after** Autumn's annotated routes.
    /// If both define the same method+path pair, Axum treats that as an
    /// overlap and router construction will fail.
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

    // ── Tier-1 subsystem replacement hooks ─────────────────────
    //
    // Each `with_*` method swaps a framework-default subsystem for a
    // user-provided trait impl. The defaults preserve current behaviour, so
    // applications that don't customize see no change. Plugins typically chain
    // these in their `build()` body to ship a subsystem (e.g. an
    // `AwsSecretsConfigPlugin` that calls `app.with_config_loader(...)`).
    // See `docs/guides/extensibility.md`.

    /// Install a custom [`ConfigLoader`],
    /// replacing the default TOML + env loader.
    ///
    /// Useful when your config lives somewhere other than `autumn.toml` —
    /// AWS Secrets Manager, Vault, a JSON file, an HTTP fetch, etc. Emits a
    /// `tracing::warn!` if a loader was already installed.
    #[must_use]
    pub fn with_config_loader<L>(mut self, loader: L) -> Self
    where
        L: crate::config::ConfigLoader,
    {
        if self.config_loader_factory.is_some() {
            tracing::warn!(
                "config loader replaced; the previously-installed loader was overwritten"
            );
        }
        self.config_loader_factory = Some(Box::new(move || {
            Box::pin(async move { loader.load().await })
        }));
        self
    }

    /// Install a custom [`DatabasePoolProvider`],
    /// replacing the default `deadpool + diesel-async` pool factory.
    ///
    /// Useful for adding metrics/circuit-breaker wrappers, switching to a
    /// per-shard pool, or driving a non-default backend at the same
    /// `Pool<AsyncPgConnection>` interface. Emits a `tracing::warn!` if a
    /// provider was already installed.
    #[cfg(feature = "db")]
    #[must_use]
    pub fn with_pool_provider<P>(mut self, provider: P) -> Self
    where
        P: crate::db::DatabasePoolProvider,
    {
        if self.pool_provider_factory.is_some() {
            tracing::warn!(
                "database pool provider replaced; the previously-installed provider was overwritten"
            );
        }
        self.pool_provider_factory =
            Some(Box::new(move |config: crate::config::DatabaseConfig| {
                Box::pin(async move { provider.create_pool(&config).await })
            }));
        self
    }

    /// Install a custom [`TelemetryProvider`](crate::telemetry::TelemetryProvider),
    /// replacing the default `tracing-subscriber + OTLP` initializer.
    ///
    /// Useful for shipping a Datadog tracer, Honeycomb beeline, Sentry
    /// integration, or any other observability backend. Emits a
    /// `tracing::warn!` if a provider was already installed.
    #[must_use]
    pub fn with_telemetry_provider<T>(mut self, provider: T) -> Self
    where
        T: crate::telemetry::TelemetryProvider,
    {
        if self.telemetry_provider.is_some() {
            tracing::warn!(
                "telemetry provider replaced; the previously-installed provider was overwritten"
            );
        }
        self.telemetry_provider = Some(Box::new(provider));
        self
    }

    /// Install a custom [`SessionStore`](crate::session::SessionStore),
    /// bypassing the config-driven `memory`/`redis` backend selection.
    ///
    /// Useful for backing sessions with a database, encrypted cookie store,
    /// or enterprise SSO bridge. Emits a `tracing::warn!` if a store was
    /// already installed.
    #[must_use]
    pub fn with_session_store<S>(mut self, store: S) -> Self
    where
        S: crate::session::SessionStore,
    {
        if self.session_store.is_some() {
            tracing::warn!(
                "session store replaced; the previously-installed store was overwritten"
            );
        }
        self.session_store = Some(Arc::new(store));
        self
    }

    /// Register an additional audit sink for structured audit events.
    ///
    /// Multiple calls accumulate sinks. Logged events are fanned out to all
    /// configured sinks.
    #[must_use]
    pub fn with_audit_sink<S>(mut self, sink: S) -> Self
    where
        S: crate::audit::AuditSink,
    {
        let logger = self
            .audit_logger
            .take()
            .map_or_else(crate::audit::AuditLogger::new, |logger| (*logger).clone())
            .with_sink(Arc::new(sink));
        self.audit_logger = Some(Arc::new(logger));
        self
    }

    /// Register a [`Policy`](crate::authorization::Policy)
    /// implementation for resource type `R`.
    ///
    /// Multiple policies per resource are not supported: registering
    /// `R` twice causes a startup-time panic with a clear error
    /// message.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use autumn_web::authorization::{Policy, PolicyContext};
    ///
    /// #[derive(Default)]
    /// struct PostPolicy;
    /// impl Policy<Post> for PostPolicy { /* ... */ }
    ///
    /// autumn_web::app()
    ///     .routes(routes![...])
    ///     .policy::<Post, _>(PostPolicy)
    ///     .run()
    ///     .await;
    /// ```
    #[must_use]
    pub fn policy<R, P>(mut self, policy: P) -> Self
    where
        R: Send + Sync + 'static,
        P: crate::authorization::Policy<R>,
    {
        self.policy_registrations.push(Box::new(move |registry| {
            registry.register_policy::<R, _>(policy);
        }));
        self
    }

    /// Register a [`Scope`](crate::authorization::Scope) implementation
    /// for resource type `R`. The scope filters list endpoints
    /// (`GET /<api>` for `#[repository(api = "...", scope = ...)]`)
    /// to records the current user is allowed to read.
    ///
    /// Default impls return an empty list so a missing scope opt-in
    /// fails closed.
    #[must_use]
    pub fn scope<R, S>(mut self, scope: S) -> Self
    where
        R: Send + Sync + 'static,
        S: crate::authorization::Scope<R>,
    {
        self.policy_registrations.push(Box::new(move |registry| {
            registry.register_scope::<R, _>(scope);
        }));
        self
    }

    /// Apply a [`Plugin`](crate::plugin::Plugin) to the builder.
    ///
    /// The plugin's [`build`](crate::plugin::Plugin::build) runs exactly once
    /// per [`AppBuilder`]. Registering two plugins that share a
    /// [`name`](crate::plugin::Plugin::name) is a no-op after the first: the
    /// duplicate emits a `tracing::warn!` and the builder is returned
    /// unchanged.
    #[must_use]
    #[track_caller]
    pub fn plugin<P>(mut self, plugin: P) -> Self
    where
        P: crate::plugin::Plugin,
    {
        let name = plugin.name();
        if self.registered_plugins.contains(name.as_ref()) {
            tracing::warn!(
                plugin = name.as_ref(),
                "plugin already registered; skipping duplicate"
            );
            return self;
        }
        self.registered_plugins.insert(name.into_owned());
        plugin.build(self)
    }

    /// Apply a [`Plugins`](crate::plugin::Plugins) bundle (a plugin or tuple
    /// of plugins) to the builder, in declaration order.
    #[must_use]
    pub fn plugins<P>(self, plugins: P) -> Self
    where
        P: crate::plugin::Plugins,
    {
        plugins.apply(self)
    }

    /// Return `true` if a plugin with the given [`Plugin::name`](crate::plugin::Plugin::name)
    /// has already been applied to this builder.
    #[must_use]
    pub fn has_plugin(&self, name: &str) -> bool {
        self.registered_plugins.contains(name)
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
            jobs,
            static_metas: _,
            exception_filters,
            scoped_groups,
            merge_routers,
            nest_routers,
            custom_layers,
            startup_hooks,
            shutdown_hooks,
            extensions: _,
            registered_plugins: _,
            error_page_renderer,
            #[cfg(feature = "db")]
            migrations,
            config_loader_factory,
            #[cfg(feature = "db")]
            pool_provider_factory,
            telemetry_provider,
            session_store,
            #[cfg(feature = "openapi")]
            openapi,
            audit_logger,
            policy_registrations,
        } = self;

        let all_routes = routes;

        // 1 & 2. Load configuration and initialize logging/telemetry
        let (config, _telemetry_guard) =
            load_config_and_telemetry(config_loader_factory, telemetry_provider).await;

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

        // 4c. Fail-fast on invalid session config — but only when no custom
        // SessionStore was installed via with_session_store(...). Done before
        // setup_database so a doomed boot doesn't run migrations first.
        fail_fast_on_invalid_session_config(&config, session_store.is_some());

        // 4d. Provision the configured BlobStore *before* `setup_database`.
        // `LocalBlobStore::new` does real IO (creates + canonicalizes the
        // root) and the storage code may `process::exit(1)` on failure
        // (unwritable root, or `storage.backend = "s3"` while the SDK
        // wiring is still tracked in #530). Doing it before migrations
        // means a doomed boot can't mutate the DB schema first.
        #[cfg(feature = "storage")]
        let storage_bootstrap = preflight_storage(&config);

        // 5. Create database pool and run migrations (if configured)
        #[cfg(feature = "db")]
        let pool = setup_database(&config, migrations, pool_provider_factory)
            .await
            .unwrap_or_else(|e| {
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

        // 5b. Fail-fast on `#[repository(api = ...)]` endpoints that
        // were mounted without a paired `policy = ...` argument when
        // running in `prod` profile and the explicit escape hatch is
        // off. Hides exactly the footgun called out in the issue:
        // "a developer who flips the `api =` switch on a
        // `#[repository]` exposes mutate endpoints that any
        // authenticated user can call against any record."
        validate_repository_api_policies(&all_routes, &scoped_groups, &config);

        // 6. Build the router (with optional static-file layer)
        let state = build_state(
            &config,
            #[cfg(feature = "db")]
            pool,
        );
        // Apply deferred policy / scope registrations onto the live
        // app state. Done before the router is built so any panic
        // from double-registration surfaces during startup, not
        // mid-request.
        for register in policy_registrations {
            register(state.policy_registry());
        }
        // Now that registrations have been applied, verify that
        // every `#[repository(policy = X)]`-annotated route has
        // an X actually registered on the live registry. Catches
        // the "wired the macro arg, forgot the `.policy(...)`
        // builder call" footgun before any 500 lands.
        validate_repository_policies_registered(&all_routes, &scoped_groups, &state, &config);
        if let Some(logger) = audit_logger {
            state.insert_extension::<crate::audit::AuditLogger>((*logger).clone());
        }

        // Install the preflighted blob store on the freshly-built
        // AppState, and remember the serving router so it gets merged
        // into the user's router below.
        #[cfg(feature = "storage")]
        let storage_router = storage_bootstrap.and_then(|b| b.install(&state));

        let env = crate::config::OsEnv;
        let dist_dir = project_dir("dist", &env);
        let dist_ref = if dist_dir.exists() {
            Some(dist_dir.as_path())
        } else {
            None
        };
        #[cfg_attr(not(feature = "storage"), allow(unused_mut))]
        let mut merge_routers = merge_routers;
        #[cfg(feature = "storage")]
        if let Some(router) = storage_router {
            merge_routers.push(router);
        }
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
                custom_layers,
                error_page_renderer,
                session_store,
                #[cfg(feature = "openapi")]
                openapi,
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
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
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
            if jobs.is_empty() {
                crate::job::clear_global_job_client();
            } else {
                crate::job::start_runtime(jobs, &state, &server_shutdown, &config.jobs);
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
            jobs: _,
            static_metas,
            exception_filters: _,
            scoped_groups: _,
            merge_routers: _,
            nest_routers: _,
            custom_layers,
            startup_hooks: _,
            shutdown_hooks: _,
            extensions: _,
            registered_plugins: _,
            error_page_renderer: _,
            #[cfg(feature = "db")]
                migrations: _,
            config_loader_factory,
            #[cfg(feature = "db")]
            pool_provider_factory,
            telemetry_provider,
            session_store,
            #[cfg(feature = "openapi")]
                openapi: _,
            audit_logger: _,
            policy_registrations,
        } = self;

        let all_routes = routes;

        // Load config (same as normal startup)
        let (config, _telemetry_guard) =
            load_config_and_telemetry(config_loader_factory, telemetry_provider).await;

        if static_metas.is_empty() {
            eprintln!("No static routes registered. Nothing to build.");
            eprintln!("Hint: use .static_routes(static_routes![...]) on your AppBuilder.");
            std::process::exit(1);
        }

        // Fail-fast on invalid session config — only when no custom store
        // was installed. Symmetrical to the same check in run() so static
        // builds don't run migrations against a doomed boot either.
        fail_fast_on_invalid_session_config(&config, session_store.is_some());

        // Preflight the configured BlobStore the same way `run()` does.
        // Static routes can read presigned URLs out of `BlobStoreState`
        // during pre-rendering (e.g. `<img src=blob.url()>`); without
        // the bootstrap they'd 500 during `autumn build` even though
        // the server path works.
        #[cfg(feature = "storage")]
        let storage_bootstrap = preflight_storage(&config);

        // Build state (with DB if configured)
        #[cfg(feature = "db")]
        let pool = setup_database(&config, vec![], pool_provider_factory)
            .await
            .unwrap_or_else(|e| {
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

        // Apply deferred policy / scope registrations onto the live
        // app state — same as `run()`. Static routes can carry
        // `#[authorize]` checks or live behind `#[repository(policy =
        // ..., scope = ...)]` index endpoints; without registering
        // here, every such pre-render call would 500 at build time
        // with `no policy/scope registered`, and `render_static_routes`
        // would treat that as a build failure even though
        // `.policy(...)` / `.scope(...)` was configured on the
        // builder.
        for register in policy_registrations {
            register(state.policy_registry());
        }

        // Install the preflighted storage and remember the serving
        // router so static generation hits the same `/_blobs/...`
        // routes the server path serves.
        #[cfg(feature = "storage")]
        let storage_router = storage_bootstrap.and_then(|b| b.install(&state));

        // Build the full router (same as production). Use the inner builder
        // so the custom session store installed via with_session_store(...)
        // is honored during static generation — apps that swap in a custom
        // store specifically to avoid Redis/external backends at build time
        // would otherwise silently fall back to the config-driven backend.
        // Custom Tower layers registered via .layer(...) are likewise
        // applied so static output matches the production response pipeline.
        #[cfg_attr(not(feature = "storage"), allow(unused_mut))]
        let mut merge_routers: Vec<axum::Router<AppState>> = Vec::new();
        #[cfg(feature = "storage")]
        if let Some(router) = storage_router {
            merge_routers.push(router);
        }
        let router = crate::router::try_build_router_inner(
            all_routes,
            &config,
            state,
            crate::router::RouterContext {
                exception_filters: Vec::new(),
                scoped_groups: Vec::new(),
                merge_routers,
                nest_routers: Vec::new(),
                custom_layers,
                error_page_renderer: None,
                session_store,
                #[cfg(feature = "openapi")]
                openapi: None,
            },
        )
        .unwrap_or_else(|error| {
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
        let schedule_desc = task_info.schedule.to_string();
        tracing::info!(name = %task_info.name, schedule = %schedule_desc, "Registered task");
    }

    let mut cron_tasks: Vec<(String, String, Option<String>, crate::task::TaskHandler)> =
        Vec::new();

    for task_info in tasks {
        let state = state.clone();
        let name = task_info.name.clone();
        let handler = task_info.handler;
        let schedule_desc = task_info.schedule.to_string();

        match task_info.schedule {
            crate::task::Schedule::FixedDelay(delay) => {
                // Register with the task registry for /actuator/tasks
                state.task_registry.register(&name, &schedule_desc);

                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(delay).await;
                        execute_fixed_delay_task(name.clone(), state.clone(), handler).await;
                    }
                });
            }
            crate::task::Schedule::Cron {
                expression,
                timezone,
            } => {
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

#[allow(unused_variables, clippy::needless_pass_by_value)]
fn send_ws_sys_task_msg(
    state: &AppState,
    event: &str,
    name: &str,
    extra: Vec<(&str, serde_json::Value)>,
) {
    #[cfg(feature = "ws")]
    {
        // ⚡ Bolt Optimization:
        // Use serde_json::json! to avoid multiple String allocations (`.to_string()`)
        // and repetitive `Map::insert` calls for `sys:tasks` websocket messages.
        let mut msg = serde_json::json!({
            "event": event,
            "task": name,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });
        if let Some(map) = msg.as_object_mut() {
            for (k, v) in extra {
                map.insert(k.to_string(), v);
            }
        }
        let _ = state.channels().sender("sys:tasks").send(msg.to_string());
    }
}

async fn execute_task_result(
    state: &AppState,
    handler: crate::task::TaskHandler,
    start: std::time::Instant,
    name: &str,
    schedule: &'static str,
) -> Result<u64, (u64, String)> {
    // A fresh span per run so OTLP-enabled deployments see each invocation
    // as its own trace rather than inheriting whatever was current on the
    // scheduler thread.
    let task_span = tracing::info_span!(
        parent: None,
        "scheduled_task",
        otel.kind = "internal",
        task = %name,
        schedule = schedule,
    );
    let result = (handler)(state.clone()).instrument(task_span).await;
    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    match result {
        Ok(()) => Ok(duration_ms),
        Err(e) => Err((duration_ms, e.to_string())),
    }
}

/// Handle the execution of a single fixed-delay task.
async fn execute_fixed_delay_task(
    name: String,
    state: AppState,
    handler: crate::task::TaskHandler,
) {
    tracing::debug!(task = %name, "Running scheduled task");
    state.task_registry.record_start(&name);

    send_ws_sys_task_msg(&state, "started", &name, vec![]);

    let start = std::time::Instant::now();
    match execute_task_result(&state, handler, start, &name, "fixed_delay").await {
        Ok(duration_ms) => {
            state.task_registry.record_success(&name, duration_ms);
            tracing::debug!(task = %name, "Task completed");
            send_ws_sys_task_msg(
                &state,
                "success",
                &name,
                vec![("duration_ms", serde_json::json!(duration_ms))],
            );
        }
        Err((duration_ms, error_str)) => {
            state
                .task_registry
                .record_failure(&name, duration_ms, &error_str);
            tracing::warn!(task = %name, error = %error_str, "Task failed");
            send_ws_sys_task_msg(
                &state,
                "failure",
                &name,
                vec![
                    ("duration_ms", serde_json::json!(duration_ms)),
                    ("error", serde_json::json!(error_str)),
                ],
            );
        }
    }
}

/// Handle the execution of a single cron task.
async fn execute_cron_task(name: String, state: AppState, handler: crate::task::TaskHandler) {
    tracing::debug!(task = %name, "Running cron task");
    state.task_registry.record_start(&name);

    send_ws_sys_task_msg(&state, "started", &name, vec![]);

    let start = std::time::Instant::now();
    match execute_task_result(&state, handler, start, &name, "cron").await {
        Ok(duration_ms) => {
            state.task_registry.record_success(&name, duration_ms);
            tracing::debug!(task = %name, "Cron task completed");
            send_ws_sys_task_msg(
                &state,
                "success",
                &name,
                vec![("duration_ms", serde_json::json!(duration_ms))],
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
                vec![
                    ("duration_ms", serde_json::json!(duration_ms)),
                    ("error", serde_json::json!(error_str)),
                ],
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

/// Fail the boot fast (before any DB side effects) when the default
/// session backend is misconfigured.
///
/// `AutumnConfig::validate()` is intentionally session-agnostic so that a
/// custom [`SessionStore`](crate::session::SessionStore) installed via
/// [`AppBuilder::with_session_store`] can override an otherwise-invalid
/// `session.backend = "redis"`-without-`redis.url` config. But when no
/// custom store is installed, the config-driven path will fail later in
/// `apply_session_layer` — and by then, `setup_database` has already run
/// migrations, leaving DB side effects from a doomed boot. This helper
/// runs the same `backend_plan` check `apply_session_layer` does, but
/// before any side effects, and only when the override path is inactive.
fn fail_fast_on_invalid_session_config(config: &AutumnConfig, has_custom_session_store: bool) {
    if has_custom_session_store {
        return;
    }
    if let Err(error) = config.session.backend_plan(config.profile.as_deref()) {
        eprintln!("Invalid session backend config: {error}");
        std::process::exit(1);
    }
}

/// Constructed [`BlobStore`](crate::storage::BlobStore) plus the
/// optional axum router that serves signed URLs for the Local backend.
/// Returned by [`preflight_storage`] before any DB side effects so a
/// doomed boot can't run migrations first; installed onto
/// [`AppState`] later via [`StorageBootstrap::install`].
#[cfg(feature = "storage")]
struct StorageBootstrap {
    store: crate::storage::SharedBlobStore,
    serving: Option<axum::Router<AppState>>,
}

#[cfg(feature = "storage")]
impl StorageBootstrap {
    /// Install the preflighted store on `AppState` and return the
    /// optional serving router so the caller can merge it into the
    /// app router.
    fn install(self, state: &AppState) -> Option<axum::Router<AppState>> {
        state.insert_extension::<crate::storage::BlobStoreState>(
            crate::storage::BlobStoreState::new(self.store),
        );
        self.serving
    }
}

/// Provision the configured [`BlobStore`](crate::storage::BlobStore)
/// before any database side effects. Construction is the side-effecting
/// step (creates + canonicalizes the storage root, may
/// `process::exit(1)` on a misconfiguration); we deliberately run it
/// before `setup_database` so a doomed boot doesn't apply migrations
/// first. Installation onto `AppState` happens later via
/// [`StorageBootstrap::install`].
#[cfg(feature = "storage")]
#[allow(clippy::too_many_lines)] // Single switch over backend variants reads as one unit.
fn preflight_storage(config: &AutumnConfig) -> Option<StorageBootstrap> {
    use crate::storage::{LocalBlobStore, SharedBlobStore, StorageBackendPlan, local::SigningKey};

    let plan = config
        .storage
        .backend_plan(config.profile.as_deref())
        .unwrap_or_else(|error| {
            // Cover the cases `backend_plan` rejects up front:
            // `LocalInProduction` (prod + local without ack),
            // `MissingS3Bucket`/`MissingS3Region`/`S3FeatureDisabled`.
            // Each is a configuration mistake — fail the boot loudly
            // rather than running migrations and then dying.
            tracing::error!(%error, "invalid storage backend config; aborting startup");
            std::process::exit(1);
        });

    match plan {
        StorageBackendPlan::Disabled => None,
        StorageBackendPlan::Local {
            provider_id,
            root,
            mount_path,
            default_url_expiry_secs,
            warn_in_production,
        } => {
            if warn_in_production {
                tracing::warn!(
                    "prod profile is using the local-disk blob store; \
                     bytes won't survive replica turnover. Set \
                     storage.backend=s3 or storage.allow_local_in_production=true \
                     to acknowledge"
                );
            }
            let signing_key = config
                .storage
                .local
                .signing_key
                .as_deref()
                .filter(|s| !s.is_empty())
                .map_or_else(
                    || {
                        if matches!(config.profile.as_deref(), Some("prod" | "production")) {
                            tracing::warn!(
                                "no storage.local.signing_key configured in prod; \
                                 generated URLs won't survive a process restart. \
                                 Set [storage.local].signing_key or \
                                 AUTUMN_STORAGE__LOCAL__SIGNING_KEY"
                            );
                        }
                        SigningKey::random()
                    },
                    |s| SigningKey::new(s.as_bytes().to_vec()),
                );
            let store = match LocalBlobStore::new(
                provider_id.clone(),
                root.clone(),
                mount_path.clone(),
                std::time::Duration::from_secs(default_url_expiry_secs),
                signing_key,
            ) {
                Ok(store) => store,
                Err(err) => {
                    // The operator explicitly chose `storage.backend = "local"`
                    // — a non-writable root means uploads can't possibly
                    // work, so abort the boot rather than letting upload
                    // handlers serve 500s after deploy.
                    tracing::error!(
                        error = %err,
                        root = %root.display(),
                        "failed to initialize local blob store; aborting startup"
                    );
                    std::process::exit(1);
                }
            };
            let serving = crate::storage::local::serve_router(&store);
            let arc: SharedBlobStore = std::sync::Arc::new(store);
            tracing::info!(
                provider = %provider_id,
                root = %root.display(),
                mount = %mount_path,
                "Local blob store mounted"
            );
            Some(StorageBootstrap {
                store: arc,
                serving: Some(serving),
            })
        }
        StorageBackendPlan::S3 {
            provider_id,
            bucket,
            region,
            endpoint,
            public_base_url,
            force_path_style,
            default_url_expiry_secs,
        } => {
            // The `storage-s3` shell ships the trait surface and config
            // story but no on-the-wire SDK — every op returns
            // `Unsupported`. Booting "successfully" here would let an
            // app with `storage.backend = "s3"` reach production and
            // only fail on first upload. Fail-fast instead, the same
            // way `local`-without-acknowledgement does. Tracked in
            // https://github.com/madmax983/autumn/issues/530.
            let _ = (
                provider_id,
                bucket,
                region,
                endpoint,
                public_base_url,
                force_path_style,
                default_url_expiry_secs,
            );
            #[cfg(feature = "storage-s3")]
            {
                tracing::error!(
                    "storage.backend=s3 is not yet implemented in autumn-web — \
                     the storage-s3 stub returns Unsupported on every operation. \
                     Wait for the autumn-storage-s3 plugin (issue #530), or pick \
                     storage.backend=local with allow_local_in_production=true \
                     for single-replica deployments. Aborting startup."
                );
            }
            #[cfg(not(feature = "storage-s3"))]
            {
                tracing::error!(
                    "storage.backend=s3 selected but the `storage-s3` cargo feature \
                     is not enabled. Aborting startup."
                );
            }
            std::process::exit(1);
        }
    }
}

async fn load_config_and_telemetry(
    config_loader: Option<ConfigLoaderFactory>,
    telemetry_provider: Option<Box<dyn crate::telemetry::TelemetryProvider>>,
) -> (AutumnConfig, crate::telemetry::TelemetryGuard) {
    // 1. Load configuration via the installed loader, falling back to the
    //    five-layer TOML + env default.
    let config = match config_loader {
        Some(factory) => factory().await,
        None => crate::config::TomlEnvConfigLoader::new().load().await,
    }
    .unwrap_or_else(|e| {
        eprintln!("Failed to load configuration: {e}");
        std::process::exit(1);
    });

    // 2. Initialize logging/telemetry via the installed provider, falling
    //    back to the default `tracing-subscriber + OTLP` initializer.
    let provider: Box<dyn crate::telemetry::TelemetryProvider> = telemetry_provider
        .unwrap_or_else(|| Box::new(crate::telemetry::TracingOtlpTelemetryProvider::new()));
    let telemetry_guard = provider
        .init(&config.log, &config.telemetry, config.profile.as_deref())
        .unwrap_or_else(|error| {
            eprintln!("Failed to initialize telemetry: {error}");
            std::process::exit(1);
        });

    (config, telemetry_guard)
}

#[cfg(feature = "db")]
async fn setup_database(
    config: &AutumnConfig,
    migrations: Vec<crate::migrate::EmbeddedMigrations>,
    pool_provider: Option<PoolProviderFactory>,
) -> Result<
    Option<diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>,
    String,
> {
    let pool = match pool_provider {
        Some(factory) => factory(config.database.clone()).await,
        None => {
            crate::db::DieselDeadpoolPoolProvider::new()
                .create_pool(&config.database)
                .await
        }
    }
    .map_err(|e| format!("Failed to create database pool: {e}"))?;

    // Skip migrations when the provider opted out of a database (returned
    // `Ok(None)`) — even if `database.url` is configured. Custom providers
    // signal "this app runs without a DB" by returning None; running
    // migrations against the URL anyway would defeat the opt-out.
    if pool.is_some() {
        if let Some(url) = &config.database.url {
            for mig in migrations {
                crate::migrate::auto_migrate(
                    url,
                    config.profile.as_deref(),
                    config.database.auto_migrate_in_production,
                    mig,
                );
            }
        }
    }

    Ok(pool)
}

/// Refuse to start when a `#[repository(api = ...)]`-mounted route
/// has no paired `policy = ...` argument in `prod` profile builds.
///
/// The issue text spells out the rationale: silently shipping
/// auto-generated CRUD endpoints with no record-level authz is a
/// security regression. The escape hatch is
/// `[security] allow_unauthorized_repository_api = true`.
/// Pure offender-collection logic for
/// [`validate_repository_api_policies`].
///
/// Walks both top-level routes and routes registered under
/// `.scoped(prefix, layer, routes)` groups, returning every
/// `#[repository(api = ...)]`-mounted *mutating* route that has no
/// paired `policy = ...` argument. Read-only mounts (GET
/// `*_api_list` / `*_api_get`) are intentionally excluded — they
/// don't fit the "any authenticated user can write to any record"
/// footgun the issue calls out. Read-leak concerns are handled
/// separately by `scope = ...`.
///
/// Returned in (resource type name, api path) form, deduped per
/// `(type, path)` pair so a repository with multiple unguarded
/// methods only shows up once.
fn collect_unguarded_repository_writes(
    routes: &[Route],
    scoped_groups: &[ScopedGroup],
) -> Vec<(String, String)> {
    let mut offenders: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<(&'static str, &'static str)> =
        std::collections::HashSet::new();
    let mut record_route = |route: &Route| {
        if let Some(meta) = route.repository {
            if !meta.has_policy
                && is_mutating_method(&route.method)
                && seen.insert((meta.resource_type_name, meta.api_path))
            {
                offenders.push((meta.resource_type_name.to_owned(), meta.api_path.to_owned()));
            }
        }
    };
    for route in routes {
        record_route(route);
    }
    for group in scoped_groups {
        for route in &group.routes {
            record_route(route);
        }
    }
    offenders
}

/// Format a list of `(type, path)` offenders into the bulleted
/// listing the startup tracing emits. Pure so the format string
/// can be unit-tested without going through `tracing` machinery.
fn format_unguarded_repository_listing(offenders: &[(String, String)]) -> String {
    offenders
        .iter()
        .map(|(name, path)| format!("  - #[repository({name}, api = \"{path}\")]"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_repository_api_policies(
    routes: &[Route],
    scoped_groups: &[ScopedGroup],
    config: &AutumnConfig,
) {
    let profile = config.profile.as_deref().unwrap_or("default");
    let strict =
        is_production_profile(profile) && !config.security.allow_unauthorized_repository_api;

    let offenders = collect_unguarded_repository_writes(routes, scoped_groups);
    if offenders.is_empty() {
        return;
    }

    let listing = format_unguarded_repository_listing(&offenders);

    if strict {
        tracing::error!(
            "refusing to start: the following #[repository(api = ...)] mutating endpoints have no paired `policy = ...` argument:\n{listing}\n\
             Add `policy = SomePolicy` to each, or set `[security] allow_unauthorized_repository_api = true` to opt out explicitly."
        );
        std::process::exit(1);
    } else {
        tracing::warn!(
            "the following #[repository(api = ...)] mutating endpoints have no paired `policy = ...` argument; \
             auto-generated POST/PUT/PATCH/DELETE handlers will accept writes from any authenticated user:\n{listing}\n\
             This will become a startup-time error in `prod` profile builds."
        );
    }
}

/// Refuse to start when a `#[repository(policy = X)]`-annotated
/// route exists but the corresponding `.policy::<R, _>(X)`
/// registration was never actually applied to the live
/// [`PolicyRegistry`].
///
/// `validate_repository_api_policies` runs *before* the registry is
/// populated and only checks the macro-set `has_policy` flag. This
/// runs *after* registrations are applied and walks the same routes,
/// invoking the macro-emitted `policy_check` probe to confirm the
/// policy is really there. Without this, forgetting the
/// `.policy::<R, _>(...)` builder call would compile, boot, and
/// then 500 on every protected request.
/// `(resource_type_name, api_path)` pair identifying a repository
/// route that's missing its required runtime registration.
type MissingRepositoryRegistration = (String, String);

/// Pure offender-collection logic for
/// [`validate_repository_policies_registered`].
///
/// Walks the same routes + scoped groups and invokes the macro-
/// emitted `policy_check` / `scope_check` probes against the live
/// registry, returning `(missing_policies, missing_scopes)` deduped
/// per `(type, path)` pair. Pure so the listing logic can be unit-
/// tested without going through the actual `tracing::error!` +
/// `std::process::exit(1)` strict path.
fn collect_unregistered_repository_handlers(
    routes: &[Route],
    scoped_groups: &[ScopedGroup],
    registry: &crate::authorization::PolicyRegistry,
) -> (
    Vec<MissingRepositoryRegistration>,
    Vec<MissingRepositoryRegistration>,
) {
    let mut missing_policies: Vec<(String, String)> = Vec::new();
    let mut missing_scopes: Vec<(String, String)> = Vec::new();
    let mut seen_policies: std::collections::HashSet<(&'static str, &'static str)> =
        std::collections::HashSet::new();
    let mut seen_scopes: std::collections::HashSet<(&'static str, &'static str)> =
        std::collections::HashSet::new();
    let mut record_route = |route: &Route| {
        if let Some(meta) = route.repository {
            if let Some(check) = meta.policy_check {
                if !check(registry)
                    && seen_policies.insert((meta.resource_type_name, meta.api_path))
                {
                    missing_policies
                        .push((meta.resource_type_name.to_owned(), meta.api_path.to_owned()));
                }
            }
            if let Some(check) = meta.scope_check {
                if !check(registry) && seen_scopes.insert((meta.resource_type_name, meta.api_path))
                {
                    missing_scopes
                        .push((meta.resource_type_name.to_owned(), meta.api_path.to_owned()));
                }
            }
        }
    };
    for route in routes {
        record_route(route);
    }
    for group in scoped_groups {
        for route in &group.routes {
            record_route(route);
        }
    }
    (missing_policies, missing_scopes)
}

/// Format a `(type, path)` listing for missing-policy startup
/// errors. Pure so the format string can be unit-tested.
fn format_missing_policy_listing(missing: &[(String, String)]) -> String {
    missing
        .iter()
        .map(|(name, path)| {
            format!("  - #[repository({name}, api = \"{path}\", policy = ...)]: call `.policy::<{name}, _>(...)` on the app builder")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a `(type, path)` listing for missing-scope startup
/// errors. Pure so the format string can be unit-tested.
fn format_missing_scope_listing(missing: &[(String, String)]) -> String {
    missing
        .iter()
        .map(|(name, path)| {
            format!("  - #[repository({name}, api = \"{path}\", scope = ...)]: call `.scope::<{name}, _>(...)` on the app builder")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_repository_policies_registered(
    routes: &[Route],
    scoped_groups: &[ScopedGroup],
    state: &AppState,
    config: &AutumnConfig,
) {
    let profile = config.profile.as_deref().unwrap_or("default");
    let strict = is_production_profile(profile);

    let (missing_policies, missing_scopes) =
        collect_unregistered_repository_handlers(routes, scoped_groups, state.policy_registry());

    if missing_policies.is_empty() && missing_scopes.is_empty() {
        return;
    }

    if !missing_policies.is_empty() {
        let listing = format_missing_policy_listing(&missing_policies);

        if strict {
            tracing::error!(
                "refusing to start: the following #[repository] routes declare a `policy = ...` argument, but no policy is registered for the resource type. Without registration, every protected request would fail at runtime with `500 no policy registered`:\n{listing}"
            );
        } else {
            tracing::warn!(
                "the following #[repository] routes declare `policy = ...` but no matching `.policy::<R, _>(...)` registration is on the app builder. Protected requests will 500 at runtime:\n{listing}\n\
                 This will become a startup-time error in `prod` profile builds."
            );
        }
    }

    if !missing_scopes.is_empty() {
        let listing = format_missing_scope_listing(&missing_scopes);

        if strict {
            tracing::error!(
                "refusing to start: the following #[repository] routes declare a `scope = ...` argument, but no scope is registered for the resource type. Without registration, every list request would fail at runtime with `500 missing scope registration`:\n{listing}"
            );
        } else {
            tracing::warn!(
                "the following #[repository] routes declare `scope = ...` but no matching `.scope::<R, _>(...)` registration is on the app builder. List requests will 500 at runtime:\n{listing}\n\
                 This will become a startup-time error in `prod` profile builds."
            );
        }
    }

    if strict {
        std::process::exit(1);
    }
}

const fn is_mutating_method(method: &http::Method) -> bool {
    matches!(
        *method,
        http::Method::POST | http::Method::PUT | http::Method::PATCH | http::Method::DELETE
    )
}

/// Returns `true` for the framework's accepted production profile
/// names. Mirrors the `prod | production` matching used elsewhere
/// (`app.rs::run_build_mode`, `migrate.rs::should_auto_apply`,
/// etc.) so the repository startup guards don't silently weaken in
/// deployments that pick the long-form alias.
fn is_production_profile(profile: &str) -> bool {
    matches!(profile, "prod" | "production")
}

#[cfg(test)]
mod validate_repository_api_policies_tests {
    use super::*;
    use crate::RepositoryApiMeta;

    fn build_route(
        method: http::Method,
        path: &'static str,
        meta: Option<RepositoryApiMeta>,
    ) -> Route {
        Route {
            method,
            path,
            handler: axum::routing::any(|| async { "" }),
            name: "test_route",
            api_doc: crate::openapi::ApiDoc::default(),
            repository: meta,
        }
    }

    fn unguarded(path: &'static str, type_name: &'static str) -> RepositoryApiMeta {
        RepositoryApiMeta {
            resource_type_name: type_name,
            api_path: path,
            has_policy: false,
            policy_check: None,
            scope_check: None,
        }
    }

    /// Tests in this module historically used a duplicated copy of
    /// the offender-collection logic. Now they call the production
    /// helper directly so coverage tracks the real code path.
    fn collect_offenders(routes: &[Route]) -> Vec<(String, String)> {
        collect_unguarded_repository_writes(routes, &[])
    }

    #[test]
    fn read_only_mount_without_policy_is_not_an_offender() {
        let routes = vec![
            build_route(
                http::Method::GET,
                "/api/posts",
                Some(unguarded("/api/posts", "Post")),
            ),
            build_route(
                http::Method::GET,
                "/api/posts/{id}",
                Some(unguarded("/api/posts", "Post")),
            ),
        ];
        let offenders = collect_offenders(&routes);
        assert!(
            offenders.is_empty(),
            "read-only mounts should not trigger the unauthorized-repo guard"
        );
    }

    #[test]
    fn write_mount_without_policy_is_an_offender() {
        let routes = vec![build_route(
            http::Method::POST,
            "/api/posts",
            Some(unguarded("/api/posts", "Post")),
        )];
        let offenders = collect_offenders(&routes);
        assert_eq!(offenders.len(), 1);
        assert_eq!(offenders[0].0, "Post");
        assert_eq!(offenders[0].1, "/api/posts");
    }

    #[test]
    fn mixed_mount_only_dedups_one_offender_per_repository() {
        let routes = vec![
            build_route(
                http::Method::GET,
                "/api/posts",
                Some(unguarded("/api/posts", "Post")),
            ),
            build_route(
                http::Method::POST,
                "/api/posts",
                Some(unguarded("/api/posts", "Post")),
            ),
            build_route(
                http::Method::PUT,
                "/api/posts/{id}",
                Some(unguarded("/api/posts", "Post")),
            ),
            build_route(
                http::Method::DELETE,
                "/api/posts/{id}",
                Some(unguarded("/api/posts", "Post")),
            ),
        ];
        let offenders = collect_offenders(&routes);
        assert_eq!(offenders.len(), 1);
    }

    #[test]
    fn is_mutating_method_classifies_methods() {
        assert!(is_mutating_method(&http::Method::POST));
        assert!(is_mutating_method(&http::Method::PUT));
        assert!(is_mutating_method(&http::Method::PATCH));
        assert!(is_mutating_method(&http::Method::DELETE));
        assert!(!is_mutating_method(&http::Method::GET));
        assert!(!is_mutating_method(&http::Method::HEAD));
        assert!(!is_mutating_method(&http::Method::OPTIONS));
    }

    // ── registry-aware validation (post-registration) ─────────────

    use crate::authorization::{Policy, PolicyRegistry};

    #[derive(Debug, Clone, PartialEq)]
    struct TestPost;

    #[derive(Default)]
    struct TestPostPolicy;
    impl Policy<TestPost> for TestPostPolicy {}

    fn guarded_with_check(path: &'static str, type_name: &'static str) -> RepositoryApiMeta {
        RepositoryApiMeta {
            resource_type_name: type_name,
            api_path: path,
            has_policy: true,
            policy_check: Some(|registry: &PolicyRegistry| registry.has_policy::<TestPost>()),
            scope_check: None,
        }
    }

    fn collect_missing(routes: &[Route], registry: &PolicyRegistry) -> Vec<(String, String)> {
        let (missing_policies, _) = collect_unregistered_repository_handlers(routes, &[], registry);
        missing_policies
    }

    #[test]
    fn registry_check_flags_routes_missing_their_policy_registration() {
        // Macro emits `policy = X` but no `.policy::<TestPost, _>(...)`
        // call on the builder — registry has nothing.
        let registry = PolicyRegistry::default();
        let routes = vec![build_route(
            http::Method::POST,
            "/api/posts",
            Some(guarded_with_check("/api/posts", "TestPost")),
        )];
        let missing = collect_missing(&routes, &registry);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "TestPost");
        assert_eq!(missing[0].1, "/api/posts");
    }

    #[test]
    fn registry_check_passes_when_policy_is_registered() {
        let registry = PolicyRegistry::default();
        registry.register_policy::<TestPost, _>(TestPostPolicy);
        let routes = vec![build_route(
            http::Method::POST,
            "/api/posts",
            Some(guarded_with_check("/api/posts", "TestPost")),
        )];
        let missing = collect_missing(&routes, &registry);
        assert!(missing.is_empty(), "policy is registered, no offenders");
    }

    #[test]
    fn registry_check_skips_routes_without_policy_check_fn() {
        // Routes mounted without `policy = ...` carry
        // `policy_check: None` and are not subject to this check —
        // they're handled by `validate_repository_api_policies` which
        // looks at `has_policy` instead.
        let registry = PolicyRegistry::default();
        let routes = vec![build_route(
            http::Method::POST,
            "/api/posts",
            Some(unguarded("/api/posts", "TestPost")),
        )];
        let missing = collect_missing(&routes, &registry);
        assert!(missing.is_empty());
    }

    #[test]
    fn registry_check_dedups_one_offender_per_repository() {
        let registry = PolicyRegistry::default();
        let routes = vec![
            build_route(
                http::Method::GET,
                "/api/posts",
                Some(guarded_with_check("/api/posts", "TestPost")),
            ),
            build_route(
                http::Method::POST,
                "/api/posts",
                Some(guarded_with_check("/api/posts", "TestPost")),
            ),
            build_route(
                http::Method::DELETE,
                "/api/posts/{id}",
                Some(guarded_with_check("/api/posts", "TestPost")),
            ),
        ];
        let missing = collect_missing(&routes, &registry);
        assert_eq!(missing.len(), 1);
    }

    // ── Scope registration validation ─────────────────────────────

    use crate::authorization::{BoxFuture, PolicyContext, Scope};

    #[derive(Default)]
    struct TestPostScope;
    impl Scope<TestPost> for TestPostScope {
        fn list<'a>(
            &'a self,
            _ctx: &'a PolicyContext,
            _conn: &'a mut diesel_async::AsyncPgConnection,
        ) -> BoxFuture<'a, crate::AutumnResult<Vec<TestPost>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn scope_only_meta(path: &'static str, type_name: &'static str) -> RepositoryApiMeta {
        RepositoryApiMeta {
            resource_type_name: type_name,
            api_path: path,
            has_policy: false,
            policy_check: None,
            scope_check: Some(|registry: &PolicyRegistry| registry.scope::<TestPost>().is_some()),
        }
    }

    fn collect_missing_scopes(
        routes: &[Route],
        registry: &PolicyRegistry,
    ) -> Vec<(String, String)> {
        let (_, missing_scopes) = collect_unregistered_repository_handlers(routes, &[], registry);
        missing_scopes
    }

    #[test]
    fn scope_check_flags_unregistered_scope() {
        let registry = PolicyRegistry::default();
        let routes = vec![build_route(
            http::Method::GET,
            "/api/posts",
            Some(scope_only_meta("/api/posts", "TestPost")),
        )];
        let missing = collect_missing_scopes(&routes, &registry);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "TestPost");
    }

    #[test]
    fn scope_check_passes_when_scope_is_registered() {
        let registry = PolicyRegistry::default();
        registry.register_scope::<TestPost, _>(TestPostScope);
        let routes = vec![build_route(
            http::Method::GET,
            "/api/posts",
            Some(scope_only_meta("/api/posts", "TestPost")),
        )];
        let missing = collect_missing_scopes(&routes, &registry);
        assert!(missing.is_empty());
    }

    #[test]
    fn scope_check_skips_routes_without_scope_check_fn() {
        let registry = PolicyRegistry::default();
        let routes = vec![build_route(
            http::Method::POST,
            "/api/posts",
            Some(unguarded("/api/posts", "TestPost")),
        )];
        let missing = collect_missing_scopes(&routes, &registry);
        assert!(missing.is_empty());
    }

    // ── prod / production profile parity ────────────────────────

    #[test]
    fn is_production_profile_matches_both_aliases() {
        assert!(is_production_profile("prod"));
        assert!(is_production_profile("production"));
        assert!(!is_production_profile("dev"));
        assert!(!is_production_profile("staging"));
        assert!(!is_production_profile("test"));
        assert!(!is_production_profile("default"));
        // Case-sensitive (matches the framework's elsewhere
        // matching pattern in app.rs::run_build_mode and
        // migrate.rs).
        assert!(!is_production_profile("Prod"));
        assert!(!is_production_profile("Production"));
    }

    // ── Formatter helpers ─────────────────────────────────────────

    #[test]
    fn format_unguarded_listing_renders_one_bullet_per_offender() {
        let offenders = vec![
            ("Post".to_owned(), "/api/posts".to_owned()),
            ("Comment".to_owned(), "/api/comments".to_owned()),
        ];
        let listing = format_unguarded_repository_listing(&offenders);
        assert!(listing.contains("Post"));
        assert!(listing.contains("/api/posts"));
        assert!(listing.contains("Comment"));
        assert!(listing.contains("/api/comments"));
        assert_eq!(listing.matches("\n  - ").count() + 1, 2);
    }

    #[test]
    fn format_unguarded_listing_empty_input_yields_empty_string() {
        let listing = format_unguarded_repository_listing(&[]);
        assert!(listing.is_empty());
    }

    #[test]
    fn format_missing_policy_listing_includes_policy_call_hint() {
        let missing = vec![("Post".to_owned(), "/api/posts".to_owned())];
        let listing = format_missing_policy_listing(&missing);
        assert!(listing.contains("Post"));
        assert!(listing.contains("/api/posts"));
        assert!(listing.contains(".policy::<Post, _>"));
        assert!(listing.contains("policy = ..."));
    }

    #[test]
    fn format_missing_scope_listing_includes_scope_call_hint() {
        let missing = vec![("Post".to_owned(), "/api/posts".to_owned())];
        let listing = format_missing_scope_listing(&missing);
        assert!(listing.contains("Post"));
        assert!(listing.contains("/api/posts"));
        assert!(listing.contains(".scope::<Post, _>"));
        assert!(listing.contains("scope = ..."));
    }

    // ── Scoped-groups path coverage ──────────────────────────────

    #[test]
    fn collect_unguarded_walks_scoped_groups() {
        // The scoped-group path catches `#[repository(api = ...)]`
        // mounts that live inside `.scoped(prefix, layer, routes)`.
        // Without walking them, the prod-mode guard would silently
        // miss those routes.
        let group_route = build_route(
            http::Method::POST,
            "/api/posts",
            Some(unguarded("/api/posts", "Post")),
        );
        let group = ScopedGroup {
            prefix: "/scoped".to_owned(),
            routes: vec![group_route],
            apply_layer: Box::new(|r| r),
        };
        let offenders = collect_unguarded_repository_writes(&[], std::slice::from_ref(&group));
        assert_eq!(offenders.len(), 1);
        assert_eq!(offenders[0].0, "Post");
    }

    #[test]
    fn collect_unregistered_walks_scoped_groups() {
        let group_route = build_route(
            http::Method::POST,
            "/api/posts",
            Some(guarded_with_check("/api/posts", "TestPost")),
        );
        let group = ScopedGroup {
            prefix: "/scoped".to_owned(),
            routes: vec![group_route],
            apply_layer: Box::new(|r| r),
        };
        let registry = PolicyRegistry::default();
        let (missing, _) =
            collect_unregistered_repository_handlers(&[], std::slice::from_ref(&group), &registry);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "TestPost");
    }
}

fn build_state(
    config: &AutumnConfig,
    #[cfg(feature = "db")] pool: Option<
        diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
    >,
) -> AppState {
    AppState {
        extensions: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        #[cfg(feature = "db")]
        pool,
        profile: config.profile.clone(),
        started_at: std::time::Instant::now(),
        health_detailed: config.health.detailed,
        probes: crate::probe::ProbeState::pending_startup(),
        metrics: crate::middleware::MetricsCollector::new(),
        log_levels: crate::actuator::LogLevels::new(&config.log.level),
        task_registry: crate::actuator::TaskRegistry::new(),
        job_registry: crate::actuator::JobRegistry::new(),
        config_props: crate::actuator::ConfigProperties::from_config(config),
        #[cfg(feature = "ws")]
        channels: crate::channels::Channels::new(32),
        #[cfg(feature = "ws")]
        shutdown: tokio_util::sync::CancellationToken::new(),
        policy_registry: crate::authorization::PolicyRegistry::default(),
        forbidden_response: config.security.forbidden_response,
        auth_session_key: config.auth.session_key.clone(),
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
    {
        out.push_str("\n    /static/js/htmx.min.js GET -> htmx");
        out.push_str("\n    /static/js/autumn-htmx-csrf.js GET -> htmx csrf");
    }
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
        let schedule = task.schedule.to_string();
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
        format!("{parsed_url} (pool_size={pool_size})")
    } else {
        // Fallback: If URL parsing fails, mask the entire URL string to prevent any
        // potential data exposure (e.g. if the malformed string still contained a password)
        format!("**** (pool_size={pool_size})")
    }
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
            api_doc: crate::openapi::ApiDoc {
                method: "GET",
                path,
                operation_id: name,
                success_status: 200,
                ..Default::default()
            },
            repository: None,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
            api_doc: crate::openapi::ApiDoc {
                method: "POST",
                path: "/submit",
                operation_id: "submit",
                success_status: 200,
                ..Default::default()
            },
            repository: None,
        }];
        let config = AutumnConfig::default();
        let state = AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
                api_doc: crate::openapi::ApiDoc {
                    method: "GET",
                    path: "/admin",
                    operation_id: "admin_list",
                    success_status: 200,
                    ..Default::default()
                },
                repository: None,
            },
            Route {
                method: http::Method::POST,
                path: "/admin",
                handler: axum::routing::post(|| async { "created" }),
                name: "create",
                api_doc: crate::openapi::ApiDoc {
                    method: "POST",
                    path: "/admin",
                    operation_id: "create",
                    success_status: 200,
                    ..Default::default()
                },
                repository: None,
            },
        ];
        let config = AutumnConfig::default();
        let state = AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
            crate::htmx::HTMX_JS_PATH,
            axum::routing::get(crate::router::htmx_handler),
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri(crate::htmx::HTMX_JS_PATH)
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
    async fn htmx_csrf_handler_returns_csp_compatible_javascript() {
        let app = axum::Router::new().route(
            crate::htmx::HTMX_CSRF_JS_PATH,
            axum::routing::get(crate::router::htmx_csrf_handler),
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri(crate::htmx::HTMX_CSRF_JS_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/javascript")
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let js = std::str::from_utf8(&body).expect("csrf helper should be valid utf-8");

        assert!(js.contains("htmx:configRequest"));
        assert!(js.contains("X-CSRF-Token"));
        assert!(!js.contains("<script"));
    }

    #[cfg(feature = "htmx")]
    #[tokio::test]
    async fn build_router_serves_htmx_js() {
        let router = test_router(vec![test_get_route("/dummy", "dummy")]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri(crate::htmx::HTMX_JS_PATH)
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

    #[cfg(feature = "htmx")]
    #[tokio::test]
    async fn build_router_serves_htmx_csrf_js() {
        let router = test_router(vec![test_get_route("/dummy", "dummy")]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri(crate::htmx::HTMX_CSRF_JS_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let csp = response
            .headers()
            .get("content-security-policy")
            .expect("framework JS should still receive security headers")
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self'"), "csp = {csp}");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let js = std::str::from_utf8(&body).expect("csrf helper should be valid utf-8");
        assert!(js.contains("htmx:configRequest"));
        assert!(js.contains("X-CSRF-Token"));
    }

    #[tokio::test]
    async fn build_router_serves_default_favicon_without_404() {
        let router = test_router(vec![test_get_route("/dummy", "dummy")]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri(crate::router::DEFAULT_FAVICON_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            response.headers().contains_key("content-security-policy"),
            "framework fallback responses should still receive security headers"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn build_router_does_not_override_user_favicon_route() {
        let router = test_router(vec![test_get_route(
            crate::router::DEFAULT_FAVICON_PATH,
            "favicon",
        )]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri(crate::router::DEFAULT_FAVICON_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"ok");
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
        let csp = response
            .headers()
            .get("content-security-policy")
            .expect("static-first HTML should still receive security headers")
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self'"), "csp = {csp}");
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
                    api_doc: crate::openapi::ApiDoc {
                        method: "GET",
                        path: "/about",
                        operation_id: "about",
                        success_status: 200,
                        ..Default::default()
                    },
                    repository: None,
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
                    api_doc: crate::openapi::ApiDoc {
                        method: "GET",
                        path: "/page",
                        operation_id: "page",
                        success_status: 200,
                        ..Default::default()
                    },
                    repository: None,
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
    async fn build_router_mounts_dev_reload_script_endpoint_when_enabled() {
        // The injected <script src="/__autumn/live-reload.js"> tag only works
        // under the default CSP (`script-src 'self'`) if the framework
        // actually serves the JS at that path. This guards against the
        // regression where the script endpoint is forgotten.
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
                let router = test_router(vec![test_get_route("/dummy", "dummy")]);

                let response = router
                    .oneshot(
                        Request::builder()
                            .uri("/__autumn/live-reload.js")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    response
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok()),
                    Some("application/javascript; charset=utf-8")
                );
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let js = std::str::from_utf8(&body).expect("utf-8");
                assert!(js.contains("fetch("), "js body: {js}");
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
        assert!(output.contains("nightly (cron 0 0 * * *)"));
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
    fn mask_database_url_invalid_url_fallback() {
        let masked = mask_database_url("this is completely invalid as a URL with supersecret", 10);
        assert!(masked.contains("****"));
        assert!(!masked.contains("supersecret"));
        assert!(masked.contains("pool_size=10"));
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            channels: crate::channels::Channels::new(32),
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
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
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            job_registry: crate::actuator::JobRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            channels: crate::channels::Channels::new(32),
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: crate::authorization::ForbiddenResponse::default(),
            auth_session_key: "user_id".to_owned(),
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
    async fn execute_task_result_ok_returns_duration() {
        let state = AppState::for_test();
        let handler: crate::task::TaskHandler = |_| Box::pin(async { Ok(()) });
        let start = std::time::Instant::now();
        let result =
            super::execute_task_result(&state, handler, start, "test_task", "fixed_delay").await;
        assert!(result.is_ok(), "expected Ok from successful handler");
        // duration_ms should be a reasonable value (not MAX)
        assert!(result.unwrap() < u64::MAX);
    }

    #[tokio::test]
    async fn execute_task_result_err_returns_duration_and_message() {
        let state = AppState::for_test();
        let handler: crate::task::TaskHandler =
            |_| Box::pin(async { Err(crate::AutumnError::bad_request_msg("test error")) });
        let start = std::time::Instant::now();
        let result =
            super::execute_task_result(&state, handler, start, "test_task", "fixed_delay").await;
        assert!(result.is_err(), "expected Err from failing handler");
        let (duration_ms, msg) = result.unwrap_err();
        assert!(duration_ms < u64::MAX);
        assert!(msg.contains("test error"));
    }

    #[cfg(feature = "storage")]
    mod storage_preflight {
        use super::super::{StorageBootstrap, preflight_storage};
        use crate::AppState;
        use crate::config::AutumnConfig;
        use crate::storage::{BlobStoreState, StorageBackend, StorageConfig, StorageLocalConfig};

        fn config_with_storage(storage: StorageConfig) -> AutumnConfig {
            AutumnConfig {
                profile: Some("dev".into()),
                storage,
                ..AutumnConfig::default()
            }
        }

        #[test]
        fn preflight_returns_none_when_disabled() {
            let cfg = config_with_storage(StorageConfig {
                backend: StorageBackend::Disabled,
                ..StorageConfig::default()
            });
            assert!(preflight_storage(&cfg).is_none());
        }

        #[test]
        fn preflight_provisions_local_backend_against_tempdir() {
            let dir = tempfile::tempdir().unwrap();
            let cfg = config_with_storage(StorageConfig {
                backend: StorageBackend::Local,
                local: StorageLocalConfig {
                    root: dir.path().to_path_buf(),
                    ..StorageLocalConfig::default()
                },
                ..StorageConfig::default()
            });
            let bootstrap = preflight_storage(&cfg).expect("local backend should provision");
            assert_eq!(bootstrap.store.provider_id(), "default");
            assert!(bootstrap.serving.is_some(), "local backend mounts a route");
        }

        #[tokio::test]
        async fn install_registers_blob_store_on_state() {
            let dir = tempfile::tempdir().unwrap();
            let cfg = config_with_storage(StorageConfig {
                backend: StorageBackend::Local,
                local: StorageLocalConfig {
                    root: dir.path().to_path_buf(),
                    ..StorageLocalConfig::default()
                },
                ..StorageConfig::default()
            });
            let bootstrap: StorageBootstrap = preflight_storage(&cfg).unwrap();

            let state = AppState::for_test();
            assert!(state.extension::<BlobStoreState>().is_none());
            let serving = bootstrap.install(&state);
            assert!(serving.is_some());
            assert!(state.extension::<BlobStoreState>().is_some());
        }
    }
}
