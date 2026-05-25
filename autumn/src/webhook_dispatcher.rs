//! Outbound Webhook Dispatcher Plugin.
//!
//! A plugin that connects the `Channels` subsystem to external HTTP endpoints.
//! It subscribes to a channel topic and automatically POSTs every message to a
//! registered URL.
//!
//! # Example
//!
//! ```rust,ignore
//! use autumn_web::webhook_dispatcher::WebhookDispatcherPlugin;
//!
//! autumn_web::app()
//!     .plugin(
//!         WebhookDispatcherPlugin::new()
//!             .dispatch("user_events", "https://example.com/webhooks/users")
//!             .with_secret("my_shared_secret")
//!     )
//!     .run()
//!     .await;
//! ```

use crate::app::AppBuilder;
#[cfg(feature = "ws")]
use crate::channels::ChannelMessage;
#[cfg(all(feature = "http-client", feature = "ws"))]
use crate::http_client::Client;
use crate::plugin::Plugin;
#[cfg(all(feature = "http-client", feature = "ws"))]
use crate::state::AppState;

/// Configuration for a single webhook dispatch route.
#[derive(Debug, Clone)]
pub struct WebhookDispatchRoute {
    pub topic: String,
    pub target_url: String,
    pub secret: Option<String>,
}

/// A plugin that bridges `Channels` to outbound webhooks.
pub struct WebhookDispatcherPlugin {
    routes: Vec<WebhookDispatchRoute>,
}

impl WebhookDispatcherPlugin {
    /// Create a new, empty webhook dispatcher.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Add a dispatch rule: messages on `topic` will be `POSTed` to `target_url`.
    #[must_use]
    pub fn dispatch(mut self, topic: impl Into<String>, target_url: impl Into<String>) -> Self {
        self.routes.push(WebhookDispatchRoute {
            topic: topic.into(),
            target_url: target_url.into(),
            secret: None,
        });
        self
    }

    /// Add an HMAC-SHA256 signing secret to the most recently added dispatch rule.
    /// The signature will be sent in the `Autumn-Signature` HTTP header.
    #[must_use]
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        if let Some(route) = self.routes.last_mut() {
            route.secret = Some(secret.into());
        }
        self
    }
}

impl Default for WebhookDispatcherPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for WebhookDispatcherPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        #[cfg(all(feature = "http-client", feature = "ws"))]
        {
            let routes = self.routes;
            app.on_startup(move |state| {
                let routes = routes.clone();
                async move {
                    for route in routes {
                        spawn_dispatcher(&state, route);
                    }
                    Ok(())
                }
            })
        }
        #[cfg(not(all(feature = "http-client", feature = "ws")))]
        {
            // If http-client is disabled, we can't dispatch.
            // We could emit a warning or just do nothing.
            tracing::warn!(
                "WebhookDispatcherPlugin requires the `http-client` and `ws` features to be enabled."
            );
            app
        }
    }
}

#[cfg(all(feature = "http-client", feature = "ws"))]
fn spawn_dispatcher(state: &AppState, route: WebhookDispatchRoute) {
    let mut subscriber = state.channels().subscribe(&route.topic);
    let client = Client::new();

    tokio::spawn(async move {
        tracing::info!(
            topic = %route.topic,
            target_url = %route.target_url,
            "Started webhook dispatcher"
        );

        loop {
            match subscriber.recv().await {
                Ok(msg) => {
                    let client = client.clone();
                    let route = route.clone();
                    tokio::spawn(async move {
                        dispatch_message(&client, &route, msg).await;
                    });
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(topic = %route.topic, lagged = n, "Webhook dispatcher skipped messages");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }

        tracing::info!(
            topic = %route.topic,
            "Webhook dispatcher shutting down (channel closed)"
        );
    });
}

#[cfg(all(feature = "http-client", feature = "ws"))]
async fn dispatch_message(client: &Client, route: &WebhookDispatchRoute, msg: ChannelMessage) {
    let payload = msg.as_str().as_bytes();

    let mut req = client
        .post(&route.target_url)
        .header("Content-Type", "application/json")
        .text_body(msg.as_str().to_owned());

    if let Some(secret) = &route.secret {
        let signature = crate::security::config::hmac_sha256_hex(secret.as_bytes(), payload);
        req = req.header("Autumn-Signature", format!("sha256={signature}"));
    }

    match req.send().await {
        Ok(res) if res.is_success() => {
            tracing::debug!(
                topic = %route.topic,
                target_url = %route.target_url,
                "Webhook dispatched successfully"
            );
        }
        Ok(res) => {
            tracing::warn!(
                topic = %route.topic,
                target_url = %route.target_url,
                status = %res.status(),
                "Webhook dispatch failed with error status"
            );
        }
        Err(err) => {
            tracing::error!(
                topic = %route.topic,
                target_url = %route.target_url,
                %err,
                "Webhook dispatch failed"
            );
        }
    }
}

#[cfg(all(test, feature = "http-client", feature = "ws"))]
mod tests {
    use super::*;

    // Test that the plugin compiles and can be constructed
    #[test]
    fn can_construct_plugin() {
        let plugin = WebhookDispatcherPlugin::new()
            .dispatch("events", "https://api.example.com/webhook")
            .with_secret("test_secret");

        assert_eq!(plugin.routes.len(), 1);
        assert_eq!(plugin.routes[0].topic, "events");
        assert_eq!(
            plugin.routes[0].target_url,
            "https://api.example.com/webhook"
        );
        assert_eq!(plugin.routes[0].secret.as_deref(), Some("test_secret"));
    }
}
