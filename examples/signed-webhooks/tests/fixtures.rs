use std::time::{SystemTime, UNIX_EPOCH};

use autumn_web::test::{TestApp, TestResponse};
use autumn_web::webhook::hmac_sha256_hex;
use signed_webhooks_example::{STRIPE_SECRET, config, routes};

fn unix_now() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after unix epoch")
        .as_secs();
    i64::try_from(secs).expect("current unix timestamp fits in i64")
}

fn stripe_signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let timestamp = timestamp.to_string();
    let mut signed_payload = Vec::with_capacity(timestamp.len() + 1 + body.len());
    signed_payload.extend_from_slice(timestamp.as_bytes());
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(body);
    let signature = hmac_sha256_hex(secret.as_bytes(), &signed_payload);
    format!("t={timestamp},v1={signature}")
}

fn client() -> autumn_web::test::TestClient {
    TestApp::new().config(config()).routes(routes()).build()
}

fn problem_json(response: &TestResponse, status: u16) -> serde_json::Value {
    response.assert_status(status);
    response.assert_header_contains("content-type", "application/problem+json");
    let json: serde_json::Value = response.json();
    assert_eq!(json["status"], status);
    assert!(
        json["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "Problem+JSON response should include a detail message"
    );
    assert!(
        json["instance"].as_str().is_some(),
        "Problem+JSON response should include an instance path"
    );
    assert!(
        json["request_id"].as_str().is_some(),
        "Problem+JSON response should include a request_id"
    );
    json
}

#[tokio::test]
async fn accepts_valid_stripe_fixture() {
    let now = unix_now();
    let body = br#"{"id":"evt_valid","type":"invoice.paid"}"#;

    let response = client()
        .post("/webhooks/stripe")
        .header("content-type", "application/json")
        .header(
            "stripe-signature",
            &stripe_signature(STRIPE_SECRET, now, body),
        )
        .body(body.as_slice())
        .send()
        .await;

    response.assert_ok();
    let json: serde_json::Value = response.json();
    assert_eq!(json["accepted"], true);
    assert_eq!(json["delivery_id"], "evt_valid");
    assert_eq!(json["event_type"], "invoice.paid");
}

#[tokio::test]
async fn rejects_tampered_body_fixture() {
    let now = unix_now();
    let signed_body = br#"{"id":"evt_tampered","type":"invoice.paid"}"#;
    let sent_body = br#"{"id": "evt_tampered", "type": "invoice.paid"}"#;

    let response = client()
        .post("/webhooks/stripe")
        .header("content-type", "application/json")
        .header(
            "stripe-signature",
            &stripe_signature(STRIPE_SECRET, now, signed_body),
        )
        .body(sent_body.as_slice())
        .send()
        .await;

    problem_json(&response, 401);
}

#[tokio::test]
async fn rejects_stale_timestamp_fixture() {
    let stale = unix_now() - 301;
    let body = br#"{"id":"evt_stale","type":"invoice.paid"}"#;

    let response = client()
        .post("/webhooks/stripe")
        .header("content-type", "application/json")
        .header(
            "stripe-signature",
            &stripe_signature(STRIPE_SECRET, stale, body),
        )
        .body(body.as_slice())
        .send()
        .await;

    problem_json(&response, 401);
}

#[tokio::test]
async fn rejects_bad_signature_fixture() {
    let now = unix_now();
    let body = br#"{"id":"evt_bad_sig","type":"invoice.paid"}"#;

    let response = client()
        .post("/webhooks/stripe")
        .header("content-type", "application/json")
        .header(
            "stripe-signature",
            &stripe_signature("wrong-dev-webhook-secret-32-bytes", now, body),
        )
        .body(body.as_slice())
        .send()
        .await;

    problem_json(&response, 401);
}

#[tokio::test]
async fn rejects_duplicate_delivery_fixture() {
    let now = unix_now();
    let body = br#"{"id":"evt_duplicate","type":"invoice.paid"}"#;
    let signature = stripe_signature(STRIPE_SECRET, now, body);
    let client = client();

    client
        .post("/webhooks/stripe")
        .header("content-type", "application/json")
        .header("stripe-signature", &signature)
        .body(body.as_slice())
        .send()
        .await
        .assert_ok();

    let duplicate = client
        .post("/webhooks/stripe")
        .header("content-type", "application/json")
        .header("stripe-signature", &signature)
        .body(body.as_slice())
        .send()
        .await;

    problem_json(&duplicate, 409);
}
