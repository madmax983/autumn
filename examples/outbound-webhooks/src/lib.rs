use autumn_web::prelude::*;
use autumn_web::webhook_outbound::{
    InMemoryOutboundWebhookStore, OutboundWebhookPlugin, WebhookOutboundManager,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: String,
}

#[post("/users")]
async fn create_user(
    state: State<AppState>,
    Json(payload): Json<User>,
) -> AutumnResult<Json<User>> {
    // Dispatch the outbound webhook event on "user.created"
    if let Some(manager) = state.extension::<WebhookOutboundManager>() {
        manager.dispatch(&state, "user.created", &payload).await?;
    }

    Ok(Json(payload))
}

/// Expose the routes for testing
pub fn routes() -> Vec<autumn_web::Route> {
    routes![create_user]
}

/// Helper to configure and return the AppBuilder with OutboundWebhookPlugin.
pub fn app() -> autumn_web::app::AppBuilder {
    let store = Arc::new(InMemoryOutboundWebhookStore::new());
    let plugin = OutboundWebhookPlugin::new(store).with_initial_backoff_ms(10);

    autumn_web::app().plugin(plugin).routes(routes())
}
