use autumn_web::security::CsrfConfig;
use autumn_web::security::CsrfLayer;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use tower::ServiceExt;

#[tokio::test]
async fn eris_csrf_path_traversal_bypass() {
    let config = CsrfConfig {
        enabled: true,
        exempt_paths: vec!["/api/".to_string()],
        ..Default::default()
    };

    let app = Router::new()
        .route("/submit", post(|| async { "created" }))
        .route("/api/items", post(|| async { "created" }))
        // Fallback represents Autumn's error page fallback middleware.
        // Without this explicit fallback on the raw Router in testing,
        // Axum's default 404 handler bypasses outer layers.
        .fallback(|| async { (StatusCode::NOT_FOUND, "Not Found") })
        .layer(CsrfLayer::from_config(&config));

    let malicious_req = Request::builder()
        .method("POST")
        .uri("/api/../submit")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(malicious_req).await.unwrap();

    // With path normalization inside CsrfLayer, `/api/../submit` resolves to `/submit`,
    // which DOES NOT start with the exempt path `/api/`. Therefore CSRF applies,
    // and since no token is provided, it must return FORBIDDEN.
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
