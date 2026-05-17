use crate::route::Route;
use crate::state::AppState;
use std::any::TypeId;

/// A group of routes sharing a common path prefix and middleware layer.
///
/// Created by [`AppBuilder::scoped`]. The routes are mounted under the
/// prefix with the middleware applied only to this group.
pub struct ScopedGroup {
    pub prefix: String,
    pub routes: Vec<Route>,
    /// Registration origin: user application or a named plugin.
    pub source: crate::route_listing::RouteSource,
    /// Closure that applies the layer to a sub-router.
    pub apply_layer: Box<dyn FnOnce(axum::Router<AppState>) -> axum::Router<AppState> + Send>,
}

/// A deferred router mutator that applies a user-registered
/// [`tower::Layer`] to the app-wide router.
///
/// Stored on [`AppBuilder`] by [`AppBuilder::layer`] and drained inside
/// `apply_middleware` where the final layer stack is assembled.
pub type CustomLayerApplier =
    Box<dyn FnOnce(axum::Router<AppState>) -> axum::Router<AppState> + Send>;

/// Metadata and deferred application closure for a user-registered layer.
pub struct CustomLayerRegistration {
    /// Concrete type for the registered layer.
    pub type_id: TypeId,
    /// Deferred router mutation that applies the layer.
    pub apply: CustomLayerApplier,
}

pub mod sealed {
    pub trait Sealed {}
}
