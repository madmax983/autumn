use axum::{body::Body, http::Request, Router, routing::get};
use tower::ServiceExt;
use std::net::SocketAddr;
use autumn_web::security::config::RateLimitConfig;
use autumn_web::security::rate_limit::RateLimitLayer;
use axum::extract::ConnectInfo;

#[tokio::test]
async fn xff_spoofing_bypasses_rate_limit() {
    let app = Router::new()
        .route("/", get(|| async { "OK" }))
        .layer(RateLimitLayer::from_config(&RateLimitConfig {
            enabled: true,
            requests_per_second: 0.1,
            burst: 1,
            trust_forwarded_headers: true,
        }));

    // Attacker sends request 1 with spoofed XFF
    let mut req1 = Request::builder()
        .uri("/")
        .header("X-Forwarded-For", "10.0.0.1, 192.168.1.100") // attacker spoofs 10.0.0.1, proxy appends 192.168.1.100
        .body(Body::empty())
        .unwrap();
    req1.extensions_mut().insert(ConnectInfo("127.0.0.1:8080".parse::<SocketAddr>().unwrap()));

    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), 200);

    // Attacker sends request 2 with different spoofed XFF, bypassing rate limit
    let mut req2 = Request::builder()
        .uri("/")
        .header("X-Forwarded-For", "10.0.0.2, 192.168.1.100")
        .body(Body::empty())
        .unwrap();
    req2.extensions_mut().insert(ConnectInfo("127.0.0.1:8080".parse::<SocketAddr>().unwrap()));

    let res2 = app.clone().oneshot(req2).await.unwrap();

    // In a secure implementation, this should be 429 because the real IP is 192.168.1.100
    // The rate limiter now correctly keys on the appended IP from the proxy, completely preventing XFF spoofing bypass.
    assert_eq!(res2.status(), 429); // 429 proves the fix works
}
