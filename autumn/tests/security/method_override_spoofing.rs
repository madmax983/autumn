use autumn_web::middleware::{MethodOverrideLayer, method_override_rejection_filter};
use axum::{body::Body, http::{Request, HeaderValue, StatusCode}, routing::delete, Router};
use tower::{Layer, ServiceExt};

#[tokio::test]
async fn test_x_forwarded_host_spoofing() {
    let router = Router::new()
        .route("/items/{id}", delete(|| async { "deleted" }))
        .layer(axum::middleware::from_fn(method_override_rejection_filter));
    let app = MethodOverrideLayer::new().layer(router);

    // Attacker sends a request with spoofed X-Forwarded-Host
    let mut req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://attacker.com")
        .header("host", "app.example.com")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    // Add attacker's spoofed header
    req.headers_mut().append("x-forwarded-host", HeaderValue::from_static("attacker.com"));
    // Add proxy's appended header
    req.headers_mut().append("x-forwarded-host", HeaderValue::from_static("app.example.com"));

    // Add attacker's spoofed proto
    req.headers_mut().append("x-forwarded-proto", HeaderValue::from_static("https"));
    // Add proxy's appended proto
    req.headers_mut().append("x-forwarded-proto", HeaderValue::from_static("https"));

    let response = app.oneshot(req).await.unwrap();

    // It should be 405 Method Not Allowed, because the leftmost x-forwarded-host is attacker.com
    // which shouldn't be trusted, it should use the rightmost one (app.example.com).
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn test_x_forwarded_proto_spoofing() {
    let router = Router::new()
        .route("/items/{id}", delete(|| async { "deleted" }))
        .layer(axum::middleware::from_fn(method_override_rejection_filter));
    let app = MethodOverrideLayer::new().layer(router);

    // Attacker sends a request with spoofed X-Forwarded-Proto
    let mut req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "http://app.example.com")
        .header("host", "app.example.com")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    // Attacker injects `http` to match their origin. Proxy appends `https`.
    req.headers_mut().append("x-forwarded-proto", HeaderValue::from_static("http, https"));

    let response = app.oneshot(req).await.unwrap();

    // It should be 405 Method Not Allowed, because the leftmost x-forwarded-proto is http
    // which shouldn't be trusted, it should use the rightmost one (https).
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
