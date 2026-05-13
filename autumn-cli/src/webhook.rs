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

    const fn as_slug(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::Github => "github",
            Self::Slack => "slack",
            Self::Generic => "generic",
        }
    }
}

fn fresh_sim_delivery_id(provider: WebhookProvider) -> String {
    let mut random = [0_u8; 16];
    if let Err(error) = getrandom::fill(&mut random) {
        eprintln!("Error: failed to generate webhook delivery ID: {error}");
        std::process::exit(1);
    }

    format!("sim-{}-{}", provider.as_slug(), hex::encode(random))
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
            req = req.header("X-Webhook-Delivery", fresh_sim_delivery_id(provider));
            req = req.header("X-Webhook-Event", "sim.event");
        }
        WebhookProvider::Github => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(payload_bytes);
            let result = mac.finalize();
            let signature_hex = hex::encode(result.into_bytes());

            req = req.header("X-Hub-Signature-256", format!("sha256={signature_hex}"));
            req = req.header("X-GitHub-Delivery", fresh_sim_delivery_id(provider));
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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

    fn capture_delivery_header(provider: &str, header: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind webhook capture server");
        let addr = listener.local_addr().expect("capture server local addr");

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept simulated webhook");
            let mut raw_request = Vec::new();
            let mut buffer = [0_u8; 1024];

            loop {
                let bytes_read = stream
                    .read(&mut buffer)
                    .expect("read simulated webhook request");
                if bytes_read == 0 {
                    break;
                }

                raw_request.extend_from_slice(&buffer[..bytes_read]);
                if raw_request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }

            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .expect("write simulated webhook response");

            let request = String::from_utf8_lossy(&raw_request);
            request
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case(header)
                        .then(|| value.trim().to_owned())
                })
                .unwrap_or_else(|| panic!("missing {header} header in request:\n{request}"))
        });

        let url = format!("http://{addr}/webhook");
        run_sim(provider, &url, "secret", r#"{"ok":true}"#);

        handle.join().expect("capture server should finish")
    }

    #[test]
    fn generic_sim_uses_fresh_delivery_id_per_invocation() {
        let first = capture_delivery_header("generic", "X-Webhook-Delivery");
        let second = capture_delivery_header("generic", "X-Webhook-Delivery");

        assert_ne!(
            first, second,
            "generic simulator reused a delivery ID, poisoning replay protection"
        );
        assert_ne!(first, "sim-delivery-123");
        assert_ne!(second, "sim-delivery-123");
    }

    #[test]
    fn github_sim_uses_fresh_delivery_id_per_invocation() {
        let first = capture_delivery_header("github", "X-GitHub-Delivery");
        let second = capture_delivery_header("github", "X-GitHub-Delivery");

        assert_ne!(
            first, second,
            "github simulator reused a delivery ID, poisoning replay protection"
        );
        assert_ne!(first, "sim-delivery-123");
        assert_ne!(second, "sim-delivery-123");
    }
}
