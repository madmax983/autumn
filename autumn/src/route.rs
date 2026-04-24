//! Route descriptor types used by macro-generated code.
//!
//! Each route macro ([`get`](crate::get), [`post`](crate::post), etc.)
//! generates a companion function that returns a [`Route`]. The
//! [`routes!`](crate::routes) macro collects these into a `Vec<Route>`
//! for the [`AppBuilder`](crate::app::AppBuilder).
//!
//! Users do not construct `Route` values directly -- they use the
//! proc macros and the `routes![]` collection macro.

use axum::routing::MethodRouter;
use http::Method;

use crate::openapi::ApiDoc;
use crate::state::AppState;

/// A single route binding an HTTP method + path to an Axum handler.
///
/// Created by the `__autumn_route_info_{name}()` companion functions
/// that route macros ([`get`](crate::get), [`post`](crate::post), etc.)
/// generate. Users don't construct this directly -- they use the
/// attribute macros and the [`routes!`](crate::routes) macro.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
///
/// #[get("/hello")]
/// async fn hello() -> &'static str { "hi" }
///
/// // `routes!` expands to a Vec<Route>:
/// let route_vec: Vec<autumn_web::Route> = routes![hello];
/// assert_eq!(route_vec.len(), 1);
/// ```
pub struct Route {
    /// HTTP method (`GET`, `POST`, `PUT`, `DELETE`, etc.).
    pub method: Method,

    /// URL path pattern (e.g., `"/users/{id}"`).
    pub path: &'static str,

    /// Axum [`MethodRouter`] that handles requests matching this route.
    pub handler: MethodRouter<AppState>,

    /// Handler function name, used for startup logging
    /// (e.g., `"hello"`, `"create_item"`).
    pub name: &'static str,

    /// OpenAPI metadata inferred from the handler's signature and any
    /// [`#[api_doc(...)]`](crate::api_doc) overrides. Consumed by
    /// [`AppBuilder::openapi`](crate::app::AppBuilder::openapi) when
    /// generating `/v3/api-docs`.
    pub api_doc: ApiDoc,
}
