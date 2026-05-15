use autumn_web::middleware::MethodOverrideLayer;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::delete,
};
use tower::ServiceExt;

#[tokio::test]
async fn method_override_vuln() {
    let app = Router::new()
        .route("/items", delete(|| async { "deleted" }))
        .layer(MethodOverrideLayer::new());

    let req = Request::builder()
        .method("POST")
        .uri("/items")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://app.example")
        .header("host", "app.example")
        .header("x-forwarded-host", "app.example")
        .header("x-forwarded-host", "evil.com")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "Vulnerable! Method overridden even though origin didn't match the last X-Forwarded-Host"
    );
}
