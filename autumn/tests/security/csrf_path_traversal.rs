use autumn_web::security::{CsrfConfig, CsrfLayer};
use axum::{Router, http::Request, routing::post};
use tower::ServiceExt;

#[tokio::test]
async fn eris_csrf_path_traversal_poc() {
    let app = Router::new()
        .route("/protected", post(|| async { "changed" }))
        .layer(CsrfLayer::from_config(&CsrfConfig {
            enabled: true,
            exempt_paths: vec!["/api/".to_string()],
            ..Default::default()
        }));

    let req = Request::builder()
        .method("POST")
        .uri("/api/../protected")
        .body(axum::body::Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();

    // The clean_path fix prevents the CSRF bypass, so the request is rejected by CSRF
    assert_eq!(res.status(), axum::http::StatusCode::FORBIDDEN);
}
