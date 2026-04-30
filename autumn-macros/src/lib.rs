//! # Autumn Macros
//!
//! Proc macros for the Autumn web framework.
//!
//! This crate provides:
//! - Route annotation macros (`#[get]`, `#[post]`, etc.)
//! - The `routes![]` collection macro
//! - The `#[autumn_web::main]` entry point macro (S-008)
//! - The `#[model]` attribute macro (S-018)
//!
//! Users should not depend on this crate directly — use `autumn-web` instead,
//! which re-exports everything.

mod api_doc;
mod authorize;
mod cached;
mod collect;
mod job;
mod jobs_macro;
mod mailer;
mod main_macro;
mod model;
mod oauth2_callback;
mod param_helpers;
mod parse;
mod paths_macro;
mod repository;
mod route;
mod routes_macro;
mod scheduled;
mod secured;
mod service;
mod static_route;
mod static_routes_macro;
mod tasks_macro;
mod ws;

use proc_macro::TokenStream;

/// Annotate an async function as a GET route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns a `Route` pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::get;
///
/// #[get("/hello")]
/// async fn hello() -> &'static str {
///     "Hello, Autumn!"
/// }
/// ```
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("GET", "get", attr.into(), item.into()).into()
}

/// Annotate an async function as a POST route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns a `Route` pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::post;
///
/// #[post("/items")]
/// async fn create_item() -> &'static str {
///     "created"
/// }
/// ```
#[proc_macro_attribute]
pub fn post(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("POST", "post", attr.into(), item.into()).into()
}

/// Annotate an async function as a PUT route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns a `Route` pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::put;
///
/// #[put("/items/{id}")]
/// async fn update_item() -> &'static str {
///     "updated"
/// }
/// ```
#[proc_macro_attribute]
pub fn put(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("PUT", "put", attr.into(), item.into()).into()
}

/// Annotate an async function as a DELETE route handler.
///
/// Generates a companion `__autumn_route_info_{name}()` function that
/// returns a `Route` pairing the path with an Axum
/// handler. In debug builds, `#[axum::debug_handler]` is automatically
/// applied for improved error messages. This has zero cost in release
/// builds.
///
/// # Example
///
/// ```ignore
/// use autumn_web::delete;
///
/// #[delete("/items/{id}")]
/// async fn remove_item() -> &'static str {
///     "removed"
/// }
/// ```
#[proc_macro_attribute]
pub fn delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("DELETE", "delete", attr.into(), item.into()).into()
}

/// Annotate an OAuth2/OIDC callback handler.
///
/// This is a convenience alias for `#[get(\"...\")]`, intended for OAuth
/// callback endpoints such as `/auth/github/callback`.
#[proc_macro_attribute]
pub fn oauth2_callback(attr: TokenStream, item: TokenStream) -> TokenStream {
    oauth2_callback::oauth2_callback_macro(attr.into(), item.into()).into()
}

/// Collect annotated route handlers into a `Vec<Route>`.
///
/// Each handler must have been annotated with a route macro (`#[get]`,
/// `#[post]`, etc.) which generates a companion
/// `__autumn_route_info_{name}()` function.
///
/// # Example
///
/// ```ignore
/// use autumn_web::{get, post, routes};
///
/// #[get("/hello")]
/// async fn hello() -> &'static str { "hello" }
///
/// #[post("/create")]
/// async fn create() -> &'static str { "created" }
///
/// let all_routes = routes![hello, create];
/// ```
#[proc_macro]
pub fn routes(input: TokenStream) -> TokenStream {
    routes_macro::routes_macro(input.into()).into()
}

/// Generate a `pub mod paths { ... }` block from a list of route handlers.
///
/// Each handler listed must be annotated with a route macro (`#[get]`,
/// `#[post]`, etc.).  The macro re-exports the generated
/// `__autumn_path_{name}` companion functions under their original names so
/// callers can write `paths::show_post(id)` instead of
/// `format!("/posts/{}", id)`.
///
/// # Example
///
/// ```ignore
/// use autumn_web::{get, paths, routes};
///
/// #[get("/posts/{id}")]
/// async fn show_post(_id: Path<i64>) -> &'static str { "post" }
///
/// paths![show_post];
///
/// // Generates:
/// // pub mod paths {
/// //     pub use super::__autumn_path_show_post as show_post;
/// // }
///
/// // Usage:
/// let url = paths::show_post(42i64).to_string(); // "/posts/42"
/// ```
#[proc_macro]
pub fn paths(input: TokenStream) -> TokenStream {
    paths_macro::paths_macro(input.into()).into()
}

/// Set up the async runtime for an Autumn application.
///
/// This is a thin wrapper around `#[tokio::main]`. The real
/// framework setup happens in `autumn_web::app().run()`.
///
/// # Example
///
/// ```ignore
/// #[autumn_web::main]
/// async fn main() {
///     autumn_web::app()
///         .routes(routes![hello])
///         .run()
///         .await;
/// }
/// ```
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    main_macro::main_macro(item.into()).into()
}

/// Generate `send_*` and `deliver_later_*` helpers for a mailer impl block.
#[proc_macro_attribute]
pub fn mailer(attr: TokenStream, item: TokenStream) -> TokenStream {
    mailer::mailer_macro(attr.into(), item.into()).into()
}

/// Attribute macro for Autumn database models.
///
/// Applies Diesel (`Queryable`, `Selectable`, `Insertable`) and Serde
/// (`Serialize`, `Deserialize`) derives, plus a `#[diesel(table_name)]`
/// attribute. The table name can be specified explicitly or inferred
/// from the struct name by converting `PascalCase` to `snake_case`
/// and appending `s`.
///
/// # Examples
///
/// Explicit table name:
///
/// ```ignore
/// use autumn_web::model;
///
/// #[model(table = "users")]
/// pub struct User {
///     pub id: i64,
///     pub name: String,
/// }
/// ```
///
/// Inferred table name (`BlogPost` -> `blog_posts`):
///
/// ```ignore
/// use autumn_web::model;
///
/// #[model]
/// pub struct BlogPost {
///     pub id: i64,
///     pub title: String,
/// }
/// ```
#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    model::model_macro(attr.into(), item.into()).into()
}

/// Derive a repository with CRUD operations and derived queries.
///
/// Generates a `PgXxxRepository` struct implementing the annotated trait,
/// with auto-generated CRUD methods and query-by-name derived methods.
///
/// # Examples
///
/// ```ignore
/// use autumn_web::repository;
///
/// #[repository(Post)]
/// trait PostRepository {
///     fn find_by_published(published: bool) -> Vec<Post>;
/// }
/// ```
#[proc_macro_attribute]
pub fn repository(attr: TokenStream, item: TokenStream) -> TokenStream {
    repository::repository_macro(attr.into(), item.into()).into()
}

/// Declare a scheduled background task.
///
/// # Examples
///
/// ```ignore
/// #[scheduled(every = "5m", name = "cleanup")]
/// async fn cleanup(state: AppState) -> AutumnResult<()> { Ok(()) }
///
/// #[scheduled(cron = "0 0 0 * * *", name = "nightly")]
/// async fn nightly(state: AppState) -> AutumnResult<()> { Ok(()) }
/// ```
#[proc_macro_attribute]
pub fn scheduled(attr: TokenStream, item: TokenStream) -> TokenStream {
    scheduled::scheduled_macro(attr.into(), item.into()).into()
}

/// Declare an on-demand background job.
#[proc_macro_attribute]
pub fn job(attr: TokenStream, item: TokenStream) -> TokenStream {
    job::job_macro(attr.into(), item.into()).into()
}

/// Annotate an async function as a statically pre-rendered GET route.
///
/// Like `#[get]`, this generates a route companion function. Additionally,
/// it generates a `__autumn_static_meta_{name}()` companion that registers
/// the route for static HTML generation at build time.
///
/// Phase 1: path parameters are **not** supported. Use `#[get]` for
/// parameterized routes.
///
/// # Example
///
/// ```ignore
/// use autumn_web::static_get;
///
/// #[static_get("/about")]
/// async fn about() -> &'static str {
///     "About us"
/// }
/// ```
#[proc_macro_attribute]
pub fn static_get(attr: TokenStream, item: TokenStream) -> TokenStream {
    static_route::static_get_macro(attr.into(), item.into()).into()
}

/// Collect `#[scheduled]` task handlers into a `Vec<TaskInfo>`.
///
/// ```ignore
/// let all_tasks = tasks![cleanup, nightly];
/// ```
#[proc_macro]
pub fn tasks(input: TokenStream) -> TokenStream {
    tasks_macro::tasks_macro(input.into()).into()
}

/// Collect `#[job]` handlers into a `Vec<JobInfo>`.
#[proc_macro]
pub fn jobs(input: TokenStream) -> TokenStream {
    jobs_macro::jobs_macro(input.into()).into()
}

/// Secure a route handler with authentication and optional role checks.
///
/// Applied before a route macro (`#[get]`, `#[post]`, etc.), this macro
/// injects an authentication guard at the top of the handler. The guard
/// checks the session for the configured auth key (default: `"user_id"`)
/// and, when roles are specified, verifies the user's role matches.
///
/// Returns `401 Unauthorized` if not authenticated, or `403 Forbidden`
/// if the user lacks the required role.
///
/// # Forms
///
/// - `#[secured]` -- require authentication only
/// - `#[secured("admin")]` -- require a specific role
/// - `#[secured("admin", "editor")]` -- require any of the listed roles
///
/// # Example
///
/// ```ignore
/// use autumn_web::prelude::*;
///
/// #[get("/admin")]
/// #[secured("admin")]
/// async fn admin_panel() -> AutumnResult<&'static str> {
///     Ok("welcome, admin")
/// }
/// ```
#[proc_macro_attribute]
pub fn secured(attr: TokenStream, item: TokenStream) -> TokenStream {
    secured::secured_macro(attr.into(), item.into()).into()
}

/// Enforce a record-level authorization policy on a route handler.
///
/// Resolves the `Policy`
/// registered for the named resource type and calls the matching
/// action method. Short-circuits with the configured deny response
/// (default `404`, optionally `403`) before the handler body runs.
///
/// Coexists with `#[secured]`: `#[secured]` answers "are you in?",
/// `#[authorize]` answers "are you allowed to act on *this record*?"
///
/// # Forms
///
/// ```ignore
/// // Resource arg is auto-detected by snake-cased type name (Post -> `post`).
/// #[authorize("update", resource = Post)]
/// async fn update_post(post: Post) -> AutumnResult<...> { ... }
///
/// // Explicit binding name (overrides the snake-case default).
/// #[authorize("delete", resource = Post, from = target)]
/// async fn destroy(target: Post) -> AutumnResult<...> { ... }
/// ```
#[proc_macro_attribute]
pub fn authorize(attr: TokenStream, item: TokenStream) -> TokenStream {
    authorize::authorize_macro(attr.into(), item.into()).into()
}

/// Collect `#[static_get]` handlers into a `Vec<StaticRouteMeta>`.
///
/// ```ignore
/// use autumn_web::prelude::*;
///
/// #[static_get("/about")]
/// async fn about() -> &'static str { "About" }
///
/// let metas = static_routes![about];
/// ```
#[proc_macro]
pub fn static_routes(input: TokenStream) -> TokenStream {
    static_routes_macro::static_routes_macro(input.into()).into()
}

/// Define a service for cross-model orchestration and non-DB side effects.
///
/// Generates a `XxxServiceImpl` struct with dependency injection via
/// `FromRequestParts`, so it can be used as a handler parameter just
/// like repositories.
///
/// Use `#[service]` when your logic orchestrates **multiple repositories**
/// or involves **non-DB side effects** (email, API calls, etc.).
/// For single-model CRUD and validation, use `#[repository]` instead.
///
/// # Examples
///
/// ```ignore
/// use autumn_web::service;
///
/// #[service]
/// pub trait OrderService {
///     fn deps(order_repo: PgOrderRepository, inventory_repo: PgInventoryRepository);
///
///     async fn place_order(&self, req: PlaceOrderRequest) -> AutumnResult<Order>;
/// }
///
/// // You implement the business logic:
/// impl OrderServiceImpl {
///     pub async fn place_order(&self, req: PlaceOrderRequest) -> AutumnResult<Order> {
///         let order = self.order_repo.save(&req.into()).await?;
///         self.inventory_repo.reserve(order.id).await?;
///         Ok(order)
///     }
/// }
///
/// // Then use it in handlers, just like a repository:
/// #[get("/orders/{id}")]
/// async fn get_order(svc: OrderServiceImpl) -> AutumnResult<Json<Order>> {
///     // ...
/// }
/// ```
#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    service::service_macro(attr.into(), item.into()).into()
}

/// Cache the return value of a function based on its arguments.
///
/// Wraps a function with an in-memory cache backed by a per-function
/// static `Cache` (from `autumn_web::cache::Cache`). Arguments
/// must implement `Hash + Eq + Clone`; the return type must be `Clone`.
///
/// # Attributes
///
/// | Attribute | Example | Description |
/// |-----------|---------|-------------|
/// | `ttl` | `"5m"` | Time-to-live per entry (uses `parse_duration` syntax) |
/// | `max` | `1000` | Max entries; oldest evicted on overflow |
/// | `result` | (flag) | Only cache `Ok` values; pass `Err` through uncached |
///
/// # Examples
///
/// ```ignore
/// use autumn_web::cached;
///
/// // Cache with 5-minute TTL, max 100 entries, only cache Ok values
/// #[cached(ttl = "5m", max = 100, result)]
/// async fn get_user(id: i64) -> AutumnResult<User> {
///     db.find(id).await
/// }
///
/// // Cache forever with no size limit
/// #[cached]
/// async fn get_config() -> Vec<String> {
///     load_config_from_disk()
/// }
/// ```
#[proc_macro_attribute]
pub fn cached(attr: TokenStream, item: TokenStream) -> TokenStream {
    cached::cached_macro(attr.into(), item.into()).into()
}

/// Enrich a route handler's auto-generated `OpenAPI` documentation.
///
/// Applied on top of a route macro (`#[get]`, `#[post]`, etc.), this
/// attribute lets you override or add documentation fields that cannot
/// be inferred from the handler signature (summaries, descriptions,
/// tags, custom success status codes).
///
/// The route macro consumes this attribute and folds the metadata into
/// the route's `ApiDoc`. When no route macro is applied, the attribute
/// is a no-op.
///
/// # Supported keys
///
/// | Key | Type | Effect |
/// |-----|------|--------|
/// | `summary` | string | Short one-line description |
/// | `description` | string | Longer multi-line description |
/// | `tag` | string | Single `OpenAPI` tag for grouping |
/// | `tags` | `[string, ...]` | Multiple `OpenAPI` tags |
/// | `operation_id` | string | Override the default operation id |
/// | `status` | integer | Success HTTP status code (defaults to `200`) |
/// | `hidden` | flag / bool | Exclude the route from the generated spec |
///
/// # Examples
///
/// ```ignore
/// use autumn_web::prelude::*;
///
/// #[get("/users/{id}")]
/// #[api_doc(summary = "Fetch a user by id", tag = "users")]
/// async fn get_user(Path(id): Path<i32>) -> String {
///     format!("User {id}")
/// }
///
/// #[post("/users")]
/// #[api_doc(description = "Create a new user", status = 201)]
/// async fn create_user(Json(req): Json<serde_json::Value>) -> Json<serde_json::Value> {
///     Json(req)
/// }
///
/// #[get("/internal/metrics")]
/// #[api_doc(hidden)]
/// async fn metrics() -> &'static str { "" }
/// ```
#[proc_macro_attribute]
pub fn api_doc(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Rust expands attribute macros top-down (outermost first), so if the
    // user writes
    //
    //   #[api_doc(summary = "...")]
    //   #[get("/x")]
    //   async fn handler() { ... }
    //
    // this macro fires BEFORE `#[get]` and would strip `#[api_doc]` from
    // the item — the route macro would then never see the overrides.
    //
    // To support both orderings, we detect any pending route attribute
    // (`get`, `post`, etc.) sitting below us and reorder: we remove the
    // route attribute and emit it as the NEW outermost attribute, and
    // we re-attach `#[api_doc(...)]` to the function body. Rust then
    // expands the route macro next, which finds and consumes the
    // preserved `#[api_doc]` via the usual attribute-list walk.
    api_doc_standalone(attr, item)
}

const ROUTE_ATTR_NAMES: &[&str] = &["get", "post", "put", "delete", "patch", "static_get", "ws"];

/// Return `true` when an attribute names one of the Autumn route macros.
///
/// We match on the **last** path segment so qualified forms like
/// `#[autumn_web::get("/x")]`, `#[autumn_macros::post("/x")]`, or
/// even `#[crate::get("/x")]` are recognized alongside the bare
/// `#[get("/x")]`. Unqualified identifiers are covered by the same
/// logic because their path has a single segment.
fn is_route_attribute(attr: &syn::Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .is_some_and(|name| ROUTE_ATTR_NAMES.contains(&name.as_str()))
}

fn api_doc_standalone(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr_ts: proc_macro2::TokenStream = attr.into();
    let mut input_fn: syn::ItemFn = match syn::parse(item.clone()) {
        Ok(f) => f,
        // Not a function (e.g. applied to a struct) — leave it alone so
        // the user sees the usual "expected function" error from rustc.
        Err(_) => return item,
    };

    let route_idx = input_fn.attrs.iter().position(is_route_attribute);

    let Some(idx) = route_idx else {
        // Standalone `#[api_doc]` with no paired route macro is a no-op;
        // route metadata is only emitted through route macros.
        return quote::quote! { #input_fn }.into();
    };

    let route_attr = input_fn.attrs.remove(idx);
    let preserved: syn::Attribute = syn::parse_quote! {
        #[api_doc(#attr_ts)]
    };
    input_fn.attrs.insert(0, preserved);

    quote::quote! {
        #route_attr
        #input_fn
    }
    .into()
}

/// Annotate an async function as a WebSocket route handler.
///
/// The function follows the **two-function pattern**: it runs at HTTP
/// upgrade time (with access to Axum extractors) and returns a closure
/// implementing `WsHandler` (from `autumn_web::ws::WsHandler`) that handles the live WebSocket connection.
///
/// The macro generates a GET route that performs the WebSocket upgrade,
/// so it integrates seamlessly with `routes![]`.
///
/// # Examples
///
/// ```ignore
/// use autumn_web::prelude::*;
/// use autumn_web::ws::{WebSocket, Message, WsHandler};
///
/// // Minimal echo handler
/// #[ws("/echo")]
/// async fn echo() -> impl WsHandler {
///     |mut socket: WebSocket| async move {
///         while let Some(Ok(msg)) = socket.recv().await {
///             if let Message::Text(text) = msg {
///                 socket.send(Message::Text(text)).await.ok();
///             }
///         }
///     }
/// }
///
/// // With extractors (runs before upgrade)
/// #[ws("/chat")]
/// async fn chat(state: AppState) -> impl WsHandler {
///     let channels = state.channels().clone();
///     |mut socket: WebSocket| async move {
///         // use channels + socket
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn ws(attr: TokenStream, item: TokenStream) -> TokenStream {
    ws::ws_macro(attr.into(), item.into()).into()
}
