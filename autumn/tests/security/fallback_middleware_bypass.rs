use autumn_web::security::{RateLimitConfig, RateLimitLayer};
use axum::extract::ConnectInfo;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use tower::ServiceExt;

#[tokio::test]
async fn test_fallback_middleware_bypass() {
    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: 0.0001, // very strict
        burst: 1,
        ..Default::default()
    };

    let app = Router::new()
        // The FIX: fallback is now applied BEFORE layers.
        .fallback(axum::routing::get(|| async {
            axum::http::StatusCode::NOT_FOUND
        }))
        .layer(RateLimitLayer::from_config(&config));

    let peer: SocketAddr = "198.51.100.1:2000".parse().unwrap();

    let mut req_one = Request::builder()
        .uri("/not-found")
        .body(Body::empty())
        .unwrap();
    req_one.extensions_mut().insert(ConnectInfo(peer));

    let resp_one = app.clone().oneshot(req_one).await.unwrap();
    assert_eq!(resp_one.status(), StatusCode::NOT_FOUND);

    let mut req_two = Request::builder()
        .uri("/not-found")
        .body(Body::empty())
        .unwrap();
    req_two.extensions_mut().insert(ConnectInfo(peer));

    let resp_two = app.oneshot(req_two).await.unwrap();

    // With the fix, the fallback IS protected by rate limiting, so it returns 429
    assert_eq!(resp_two.status(), StatusCode::TOO_MANY_REQUESTS);
}
