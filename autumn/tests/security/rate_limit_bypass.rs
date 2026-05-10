use autumn_web::security::{RateLimitConfig, RateLimitLayer};
use axum::extract::ConnectInfo;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use std::net::SocketAddr;
use tower::ServiceExt;

#[tokio::test]
async fn eris_rate_limit_bypass_poc() {
    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: 1.0,
        burst: 1,
        // In production setups behind a load balancer, this is enabled
        trust_forwarded_headers: true,
    };

    let app = Router::new()
        .route("/", get(|| async { "ok" }))
        .layer(RateLimitLayer::from_config(&config));

    let peer_addr: SocketAddr = "192.168.1.100:1234".parse().unwrap();

    let make_req_multi = |spoofed_xff: &str, proxy_xff: &str| {
        let mut req = Request::builder()
            .method("GET")
            .uri("/")
            .header("X-Forwarded-For", spoofed_xff)
            .body(Body::empty())
            .unwrap();
        req.headers_mut()
            .append("X-Forwarded-For", proxy_xff.parse().unwrap());
        req.extensions_mut().insert(ConnectInfo(peer_addr));
        req
    };

    // Attacker sends request 1 with spoofed IP "1.1.1.1"
    // The load balancer appends the real IP in a *second* X-Forwarded-For header
    let request_1 = make_req_multi("1.1.1.1", "203.0.113.50");
    let response_1 = app.clone().oneshot(request_1).await.unwrap();
    assert_eq!(response_1.status(), StatusCode::OK);

    // Attacker sends request 2 with spoofed IP "2.2.2.2"
    // The load balancer appends the real IP in a *second* X-Forwarded-For header
    let request_2 = make_req_multi("2.2.2.2", "203.0.113.50");
    let response_2 = app.clone().oneshot(request_2).await.unwrap();

    // If vulnerable, the rate limiter uses the spoofed IP and allows the request.
    // If fixed, the rate limiter uses the right-most (or last untrusted) IP and blocks the request.
    assert_eq!(
        response_2.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "Rate limit bypassed via spoofed X-Forwarded-For left-most IP!"
    );
}
