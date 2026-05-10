use autumn_web::security::RateLimitConfig;
use autumn_web::security::RateLimitLayer;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

// 👺 Havoc: Rate limit bypass via X-Forwarded-For spoofing
// 🌩️ **The Trigger:** An attacker appends a spoofed IP to the left of their real IP in the X-Forwarded-For header.
// 📉 **The Stack Trace:** No stack trace, but the rate limiter fails to enforce limits because it parses the leftmost IP instead of the rightmost IP.
// 🧪 **Reproduction:** See `test_rate_limit_spoofing` below.
// 😈 **Comment:** You trusted the first IP the client gave you. Never trust the client.

#[tokio::test]
async fn test_rate_limit_spoofing() {
    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: 1.0,
        burst: 1,
        trust_forwarded_headers: true,
    };

    let app = axum::Router::new()
        .route("/", axum::routing::get(|| async { "ok" }))
        .layer(RateLimitLayer::from_config(&config));

    // Send a request from "real-ip" but with a spoofed X-Forwarded-For header
    let req1 = Request::builder()
        .method("GET")
        .uri("/")
        .header("X-Forwarded-For", "spoofed-ip, real-ip")
        .body(Body::empty())
        .unwrap();

    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), axum::http::StatusCode::OK);

    // Send another request with a DIFFERENT spoofed IP but same real IP
    let req2 = Request::builder()
        .method("GET")
        .uri("/")
        .header("X-Forwarded-For", "different-spoof, real-ip")
        .body(Body::empty())
        .unwrap();

    let res2 = app.clone().oneshot(req2).await.unwrap();

    // Now that the bug is fixed, the rate limiter uses the rightmost IP ("real-ip") for BOTH requests.
    // The second request should be throttled.
    assert_eq!(
        res2.status(),
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "Spoofing failed: Rate limit enforced correctly!"
    );
}
