use hmac::{Hmac, Mac};
use reqwest::blocking::Client;
use sha2::Sha256;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookProvider {
    Stripe,
    Github,
    Slack,
    Generic,
}

impl WebhookProvider {
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "stripe" => Ok(Self::Stripe),
            "github" => Ok(Self::Github),
            "slack" => Ok(Self::Slack),
            "generic" => Ok(Self::Generic),
            _ => Err(format!(
                "Unknown provider '{s}'. Supported providers: stripe, github, slack, generic"
            )),
        }
    }
}

pub fn run_sim(provider_str: &str, url: &str, secret: &str, payload: &str) {
    let provider = match WebhookProvider::from_str(provider_str) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    println!("🌟 Simulating webhook for provider: {provider:?}");
    println!("📡 Sending to URL: {url}");

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("failed to initialize HTTP client");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is set before Unix epoch")
        .as_secs();

    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(payload.to_string());

    let payload_bytes = payload.as_bytes();

    match provider {
        WebhookProvider::Generic => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(payload_bytes);
            let result = mac.finalize();
            let signature_hex = hex::encode(result.into_bytes());

            req = req.header("X-Webhook-Signature", format!("sha256={signature_hex}"));
            req = req.header("X-Webhook-Delivery", "sim-delivery-123");
            req = req.header("X-Webhook-Event", "sim.event");
        }
        WebhookProvider::Github => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(payload_bytes);
            let result = mac.finalize();
            let signature_hex = hex::encode(result.into_bytes());

            req = req.header("X-Hub-Signature-256", format!("sha256={signature_hex}"));
            req = req.header("X-GitHub-Delivery", "sim-delivery-123");
            req = req.header("X-GitHub-Event", "sim.event");
        }
        WebhookProvider::Stripe => {
            let signed_payload = format!("{now}.{payload}");
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(signed_payload.as_bytes());
            let result = mac.finalize();
            let signature_hex = hex::encode(result.into_bytes());

            req = req.header("Stripe-Signature", format!("t={now},v1={signature_hex}"));
        }
        WebhookProvider::Slack => {
            let signed_payload = format!("v0:{now}:{payload}");
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(signed_payload.as_bytes());
            let result = mac.finalize();
            let signature_hex = hex::encode(result.into_bytes());

            req = req.header("X-Slack-Signature", format!("v0={signature_hex}"));
            req = req.header("X-Slack-Request-Timestamp", now.to_string());
        }
    }

    match req.send() {
        Ok(response) => {
            let status = response.status();
            println!("✅ Response Status: {status}");
            if let Ok(text) = response.text()
                && !text.is_empty()
            {
                println!("📝 Response Body: {text}");
            }
        }
        Err(e) => {
            eprintln!("❌ Failed to send webhook: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_from_str() {
        assert_eq!(
            WebhookProvider::from_str("stripe").unwrap(),
            WebhookProvider::Stripe
        );
        assert_eq!(
            WebhookProvider::from_str("STRIPE").unwrap(),
            WebhookProvider::Stripe
        );
        assert_eq!(
            WebhookProvider::from_str("github").unwrap(),
            WebhookProvider::Github
        );
        assert_eq!(
            WebhookProvider::from_str("slack").unwrap(),
            WebhookProvider::Slack
        );
        assert_eq!(
            WebhookProvider::from_str("generic").unwrap(),
            WebhookProvider::Generic
        );
        assert!(WebhookProvider::from_str("unknown").is_err());
    }
}
