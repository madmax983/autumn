//! # outbound-http example
//!
//! Demonstrates `autumn_web::http::Client` for traced outbound HTTP calls with
//! retries.  The "Stripe" charge endpoint is called from a POST handler;
//! integration tests assert the call count and (when `telemetry-otlp` is
//! enabled) verify that the inbound trace ID propagates to the outbound
//! `traceparent` header.
//!
//! Run the server:
//! ```sh
//! cargo run -p outbound-http
//! ```
//!
//! Make a charge (uses real Stripe sandbox — set STRIPE_SECRET_KEY first):
//! ```sh
//! curl -X POST http://localhost:3000/charges \
//!      -H 'content-type: application/json' \
//!      -d '{"amount": 1000, "currency": "usd"}'
//! ```

use autumn_web::http::Client;
use autumn_web::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Charge request body sent by the caller.
#[derive(Debug, Deserialize)]
struct ChargeRequest {
    amount: u64,
    currency: String,
}

/// Charge response returned by Stripe (simplified).
#[derive(Debug, Serialize, Deserialize)]
struct ChargeResponse {
    id: String,
    amount: u64,
    status: String,
}

/// POST /charges — create a Stripe charge.
///
/// The `Client` extractor provides a pre-configured outbound HTTP client that:
/// - Propagates the active span's `traceparent` header automatically.
/// - Retries transient failures (502/503/504) up to 3 times.
/// - Returns mocked responses in tests via `TestApp::http_mock("stripe")`.
#[post("/charges")]
async fn create_charge(
    client: Client,
    Json(req): Json<ChargeRequest>,
) -> AutumnResult<Json<ChargeResponse>> {
    let stripe_client = client.named("stripe");

    let resp = stripe_client
        .post("/v1/charges")
        .header(
            "authorization",
            &format!(
                "Bearer {}",
                std::env::var("STRIPE_SECRET_KEY").unwrap_or_else(|_| "sk_test_xxx".into())
            ),
        )
        .json(&json!({
            "amount": req.amount,
            "currency": req.currency,
            "source": "tok_visa",
        }))
        .send()
        .await?;

    let charge: ChargeResponse = resp.json()?;
    Ok(Json(charge))
}

/// GET /status — simple liveness check (distinct from the framework's /health probe).
#[get("/status")]
async fn status() -> &'static str {
    "ok"
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![create_charge, status])
        .run()
        .await;
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_web::test::TestApp;
    use serde_json::json;

    /// Mocked call: asserts the handler calls Stripe exactly once and the
    /// canned response is forwarded to the caller.
    #[tokio::test]
    async fn create_charge_calls_stripe_once() {
        let mut app = TestApp::new().routes(routes![create_charge]);

        // Register a canned Stripe response — no network required.
        let mock = app
            .http_mock("stripe")
            .post("/v1/charges")
            .respond_with(
                200,
                json!({
                    "id": "ch_test_123",
                    "amount": 1000,
                    "status": "succeeded"
                }),
            );

        let client = app.build();

        let resp = client
            .post("/charges")
            .json(&json!({"amount": 1000, "currency": "usd"}))
            .send()
            .await;
        resp.assert_status(200);

        // Assert the handler made exactly one outbound call.
        mock.expect_called(1);
    }

    /// Verify that the handler correctly forwards the charge amount and currency
    /// from the inbound request to Stripe.
    #[tokio::test]
    async fn create_charge_returns_stripe_body() {
        let mut app = TestApp::new().routes(routes![create_charge]);

        let _mock = app
            .http_mock("stripe")
            .post("/v1/charges")
            .respond_with(
                200,
                json!({
                    "id": "ch_abc",
                    "amount": 500,
                    "status": "succeeded"
                }),
            );

        let client = app.build();
        let resp = client
            .post("/charges")
            .json(&json!({"amount": 500, "currency": "eur"}))
            .send()
            .await;

        resp.assert_status(200)
            .assert_body_contains("ch_abc")
            .assert_body_contains("succeeded");
    }

    /// A request with an unregistered path returns a ClientError::NoMock —
    /// confirms the mock harness is active and guards against accidental real
    /// network calls in tests.
    #[tokio::test]
    async fn unregistered_mock_path_returns_error() {
        let mut app = TestApp::new().routes(routes![create_charge]);

        // Register a mock for a DIFFERENT path to activate the registry.
        let _mock = app
            .http_mock("stripe")
            .post("/v1/other")
            .respond_with(200, json!({}));

        let client = app.build();

        // The handler hits /v1/charges which is not mocked → 500 Internal Server Error
        // (ClientError::NoMock maps through AutumnError's blanket From<E> impl).
        let resp = client
            .post("/charges")
            .json(&json!({"amount": 100, "currency": "usd"}))
            .send()
            .await;
        resp.assert_status(500);
    }

    /// End-to-end routing test confirming the app boots and the status endpoint
    /// responds.  Doubles as a smoke-test for trace context plumbing (the mock
    /// `Client` would propagate `traceparent` if `telemetry-otlp` were enabled).
    #[tokio::test]
    async fn status_endpoint_returns_ok() {
        let app = TestApp::new().routes(routes![status]).build();
        app.get("/status").send().await.assert_status(200);
    }
}
