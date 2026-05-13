use autumn_web::security::{RateLimitConfig, RateLimitLayer};
use axum::{routing::get, Router, body::Body, extract::ConnectInfo};
use http::Request;
use std::net::SocketAddr;
use tower::ServiceExt;

#[tokio::test]
async fn x_forwarded_for_multiple_headers_bypass() {
    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: 1.0,
        burst: 1,
        trust_forwarded_headers: true,
        trusted_proxies: vec!["10.0.0.0/8".to_string()],
    };

    let app = Router::new()
        .route("/", get(|| async { "ok" }))
        .layer(RateLimitLayer::from_config(&config));

    // Request 1: Attacker sends an untrusted IP.
    // The reverse proxy appends its own IP in a separate header.
    let mut req1 = Request::builder()
        .method("GET")
        .uri("/")
        .header("X-Forwarded-For", "attacker-ip-1")
        .header("X-Forwarded-For", "192.168.1.1") // Proxy appends here (this is simulating another header line)
        .body(Body::empty())
        .unwrap();
    req1.extensions_mut().insert(ConnectInfo("10.0.0.1:4000".parse::<SocketAddr>().unwrap()));

    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), http::StatusCode::OK);

    // Request 2: Attacker sends another request but changes the *first* header.
    let mut req2 = Request::builder()
        .method("GET")
        .uri("/")
        .header("X-Forwarded-For", "attacker-ip-2")
        .header("X-Forwarded-For", "192.168.1.1")
        .body(Body::empty())
        .unwrap();
    req2.extensions_mut().insert(ConnectInfo("10.0.0.1:4000".parse::<SocketAddr>().unwrap()));

    let res2 = app.clone().oneshot(req2).await.unwrap();

    // With the fix, the joined header is "attacker-ip-2,192.168.1.1".
    // "192.168.1.1" is untrusted, so the limiter correctly uses it, and sees the same client for both req1 and req2.
    // Thus it blocks req2!
    assert_eq!(res2.status(), http::StatusCode::TOO_MANY_REQUESTS);
}
