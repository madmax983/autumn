use crate::FeatureFlagStore;
use crate::admin;
use autumn_web::AppState;
use autumn_web::prelude::*;
use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone)]
pub struct FeatureFlagUpdate {
    pub name: String,
    pub enabled: bool,
}

#[get("/api/feature-flags")]
#[allow(clippy::missing_errors_doc)]
pub async fn get_feature_flags(
    State(state): State<AppState>,
) -> AutumnResult<Json<std::collections::HashMap<String, bool>>> {
    let store = state
        .extension::<Arc<dyn FeatureFlagStore>>()
        .ok_or_else(|| {
            AutumnError::internal_server_error_msg("FeatureFlagStore not found in AppState")
        })?;
    Ok(Json(store.get_all()))
}

#[get("/api/feature-flags/stream")]
pub async fn stream_feature_flags(State(state): State<AppState>) -> impl IntoResponse {
    state.channels().sse_stream("feature-flags")
}

#[must_use]
#[allow(clippy::missing_const_for_fn)]
pub fn plugin_routes() -> Vec<autumn_web::route_listing::RouteInfo> {
    vec![] // Currently relying on main app router
}

#[must_use]
pub fn routes() -> Vec<autumn_web::Route> {
    let mut routes = routes![get_feature_flags, stream_feature_flags];
    routes.extend(admin::routes());
    routes
}
