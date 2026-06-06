use autumn_web::maintenance::{MaintenanceConfig, MaintenanceState};
use autumn_web::middleware::maintenance::MaintenanceLayer;
use autumn_web::security::TrustedProxy;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::{Router, body::Body, routing::get};
use std::net::SocketAddr;
use tower::ServiceExt;

#[tokio::test]
async fn maintenance_trusted_proxy_bypass() {
    let state = MaintenanceState::new();
    state.enable(MaintenanceConfig {
        message: Some("Maintenance Mode Active".to_string()),
        allow_ips: vec!["192.168.1.10".to_string()], // only this IP is allowed
        ..Default::default()
    });

    let trusted_proxy = TrustedProxy::parse("203.0.113.10").unwrap();

    let app = Router::new()
        .route("/", get(|| async { "Hello" }))
        .layer(
            MaintenanceLayer::new(state)
                .with_trust_forwarded_headers(true)
                .with_trusted_proxies(vec![trusted_proxy]),
        );

    let peer: SocketAddr = "203.0.113.10:4000".parse().unwrap();

    // 1. Request from allowed IP via trusted proxy
    let mut req1 = Request::builder()
        .uri("/")
        .header("X-Forwarded-For", "192.168.1.10")
        .body(Body::empty())
        .unwrap();
    req1.extensions_mut().insert(ConnectInfo(peer));

    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    // 2. Request from disallowed IP via trusted proxy
    let mut req2 = Request::builder()
        .uri("/")
        .header("X-Forwarded-For", "192.168.1.9")
        .body(Body::empty())
        .unwrap();
    req2.extensions_mut().insert(ConnectInfo(peer));

    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::SERVICE_UNAVAILABLE);

    // 3. Request from allowed IP via untrusted peer (should not be trusted)
    let peer_untrusted: SocketAddr = "203.0.113.11:4000".parse().unwrap();
    let mut req3 = Request::builder()
        .uri("/")
        .header("X-Forwarded-For", "192.168.1.10")
        .body(Body::empty())
        .unwrap();
    req3.extensions_mut().insert(ConnectInfo(peer_untrusted));

    let resp3 = app.clone().oneshot(req3).await.unwrap();
    assert_eq!(resp3.status(), StatusCode::SERVICE_UNAVAILABLE);
}
