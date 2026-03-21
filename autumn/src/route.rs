//! Route types used by macro-generated code.
//!
//! Each `#[get("/path")]` (or `#[post]`, etc.) macro generates a companion
//! function that returns a [`Route`]. The `routes![]` macro collects these
//! into a `Vec<Route>` for the app builder.

use axum::routing::MethodRouter;
use http::Method;

use crate::AppState;

/// A single route binding an HTTP method + path to an Axum handler.
///
/// Created by the `__autumn_route_info_{name}()` functions that route
/// macros generate. Users don't construct this directly — they use
/// `#[get]`, `#[post]`, etc. and the `routes![]` macro.
pub struct Route {
    /// HTTP method (GET, POST, PUT, DELETE, etc.).
    pub method: Method,

    /// URL path pattern (e.g., `"/users/{id}"`).
    pub path: &'static str,

    /// Axum method router that handles requests to this route.
    pub handler: MethodRouter<AppState>,

    /// Function name, for startup logging (e.g., `"get_user"`).
    pub name: &'static str,
}
