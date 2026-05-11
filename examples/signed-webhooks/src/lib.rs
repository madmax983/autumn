use autumn_web::prelude::*;
use autumn_web::webhook::{WebhookConfig, WebhookEndpointConfig, WebhookProvider};

pub const STRIPE_SECRET: &str = "dev-stripe-webhook-secret-32-bytes";

#[post("/webhooks/stripe")]
async fn stripe(webhook: SignedWebhook) -> Json<serde_json::Value> {
    let payload = webhook
        .json::<serde_json::Value>()
        .unwrap_or(serde_json::Value::Null);
    Json(serde_json::json!({
        "accepted": true,
        "provider": webhook.provider(),
        "delivery_id": webhook.delivery_id(),
        "event_type": webhook.event_type(),
        "payload": payload,
    }))
}

pub fn routes() -> Vec<autumn_web::Route> {
    routes![stripe]
}

pub fn config() -> autumn_web::config::AutumnConfig {
    autumn_web::config::AutumnConfig {
        profile: Some("test".to_owned()),
        security: autumn_web::security::SecurityConfig {
            csrf: autumn_web::security::CsrfConfig {
                enabled: false,
                ..Default::default()
            },
            webhooks: WebhookConfig {
                endpoints: vec![
                    WebhookEndpointConfig::new(
                        "stripe",
                        "/webhooks/stripe",
                        WebhookProvider::Stripe,
                        STRIPE_SECRET,
                    )
                    .with_timestamp_tolerance_secs(300)
                    .with_replay_window_secs(86400),
                ],
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}
