use autumn_web::middleware::MethodOverrideLayer;
use axum::{Router, routing::delete};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use axum::body::Body;

#[tokio::test]
async fn eris_xff_host_bypass_poc() {
    let app = Router::new()
        .route("/delete", delete(|| async { "deleted" }))
        .layer(MethodOverrideLayer::new());

    let mut req = Request::builder()
        .method("POST")
        .uri("/delete")
        // No sec-fetch-site, meaning it falls back to origin matching.
        .header("Origin", "https://attacker.com")
        .header("X-Forwarded-Proto", "https")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(Body::from("_method=DELETE"))
        .unwrap();
    // Attacker sets: X-Forwarded-Host: attacker.com
    req.headers_mut().append("x-forwarded-host", "attacker.com".parse().unwrap());
    // Proxy adds: X-Forwarded-Host: app.example
    req.headers_mut().append("x-forwarded-host", "app.example".parse().unwrap());

    let response = app.oneshot(req).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "VULNERABLE! Server trusted the attacker's first X-Forwarded-Host header value!"
    );
}

#[tokio::test]
async fn eris_xff_host_bypass_comma_poc() {
    let app = Router::new()
        .route("/delete", delete(|| async { "deleted" }))
        .layer(MethodOverrideLayer::new());

    let req = Request::builder()
        .method("POST")
        .uri("/delete")
        .header("Origin", "https://attacker.com")
        .header("X-Forwarded-Proto", "https")
        // A single header containing comma-separated values, which is the standard way proxies append
        .header("X-Forwarded-Host", "attacker.com, app.example")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "VULNERABLE! Server trusted the attacker's first X-Forwarded-Host header value!"
    );
}
