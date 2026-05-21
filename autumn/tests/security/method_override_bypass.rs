use autumn_web::middleware::{MethodOverrideLayer, method_override_rejection_filter};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Router, routing::delete};
use tower::{Layer, ServiceExt};

#[tokio::test]
async fn poc_method_override_x_forwarded_host_bypass() {
    let router = Router::new()
        .route("/items/1", delete(|| async { "deleted" }))
        .layer(axum::middleware::from_fn(method_override_rejection_filter));
    let app = MethodOverrideLayer::new().layer(router);

    // Provide an unexpected Host Header (to simulate bypass)
    let mut request = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.example")
        .header("host", "app.example")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    // Attacker spoofed header comes first
    request
        .headers_mut()
        .append("x-forwarded-host", "evil.example".parse().unwrap());
    // Proxy appended header comes second
    request
        .headers_mut()
        .append("x-forwarded-host", "app.example".parse().unwrap());

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "SUCCESS: Method override origin check rejected forged X-Forwarded-Host"
    );
}
