use axum::{body::Body, http::{Request, StatusCode}, Router, routing::{delete, post}};
use tower::ServiceExt;
use autumn_web::security::{CsrfLayer, CsrfConfig};

#[tokio::test]
async fn csrf_htmx_header_spoofing() {
    let csrf_config = CsrfConfig {
        enabled: true,
        ..Default::default()
    };
    let app = Router::new()
        .route("/items/{id}", delete(|| async { "delete-ok" }))
        .layer(CsrfLayer::from_config(&csrf_config));

    // Standard request WITHOUT CSRF token, but WITH an HX-Request header
    let req = Request::builder()
        .method("DELETE")
        .uri("/items/1")
        .header("hx-request", "true")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();

    // Check if the presence of HX-Request somehow bypasses the CSRF check
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "VULNERABILITY: CSRF bypassed by HX-Request header!"
    );
}
