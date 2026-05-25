use autumn_web::middleware::method_override::MethodOverrideLayer;
use axum::{Router, body::Body};
use http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn method_override_xff_spoofing() {
    let app = Router::new()
        .route("/items/1", axum::routing::delete(|| async { "DELETED" }))
        .layer(MethodOverrideLayer::new());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/items/1")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("origin", "https://attacker.example")
                .header("host", "app.example")
                .header("x-forwarded-host", "attacker.example") // Spoofed
                .header("x-forwarded-host", "app.example") // Proxy appended
                .header("x-forwarded-proto", "https")
                .body(Body::from("_method=DELETE"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Must reject because the *rightmost* X-Forwarded-Host (app.example)
    // does not match the origin (attacker.example)
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
