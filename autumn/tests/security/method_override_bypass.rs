use axum::{body::Body, http::{Request, StatusCode}, Router, routing::{delete, post}};
use tower::{ServiceExt, Layer};
use autumn_web::middleware::{MethodOverrideLayer, method_override_rejection_filter};
use autumn_web::middleware::MethodOverrideService;

fn layered_router() -> MethodOverrideService<Router> {
    let router = Router::new()
        .route("/items/{id}", post(|| async { "post-ok" }))
        .route("/items/{id}", delete(|| async { "delete-ok" }))
        .layer(axum::middleware::from_fn(method_override_rejection_filter));
    MethodOverrideLayer::new().layer(router)
}

#[tokio::test]
async fn attacker_spoofed_x_forwarded_host_multiple_headers() {
    let app = layered_router();
    let mut req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.example")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    // Attacker sends their own header
    req.headers_mut().append("x-forwarded-host", "evil.example".parse().unwrap());
    // Proxy appends the real one
    req.headers_mut().append("x-forwarded-host", "app.example".parse().unwrap());

    // Attacker sends their own proto
    req.headers_mut().append("x-forwarded-proto", "https".parse().unwrap());

    let response = app.oneshot(req).await.unwrap();

    // If it succeeds, the exploit worked and it returned 200 "delete-ok"
    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "VULNERABILITY: Spoofed X-Forwarded-Host was accepted!"
    );
}

#[tokio::test]
async fn attacker_spoofed_x_forwarded_host_comma_separated() {
    let app = layered_router();
    let req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.example")
        .header("x-forwarded-host", "evil.example, app.example")
        .header("x-forwarded-proto", "https, http")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();

    // If it succeeds, the exploit worked and it returned 200 "delete-ok"
    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "VULNERABILITY: Spoofed comma-separated X-Forwarded-Host was accepted!"
    );
}
