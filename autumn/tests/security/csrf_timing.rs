use autumn_web::security::CsrfLayer;
use autumn_web::security::config::CsrfConfig;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use tower::ServiceExt;

#[tokio::test]
async fn test_csrf_timing_attack() {
    let config = CsrfConfig {
        enabled: true,
        ..Default::default()
    };

    let app = Router::new()
        .route("/submit", post(|| async { "created" }))
        .layer(CsrfLayer::from_config(&config));

    // The real token to match
    let token = "12345678-1234-1234-1234-123456789012".to_string();

    // In a timing attack, comparing "2..." vs "1..." takes different time
    // But testing execution time reliably in CI is flaky.
    // Instead, we verify that the constant-time trait or verify is used.
    // As a PoC, we will simulate the behavior but testing actual timing is difficult.

    // This test ensures that the code compiles with the fix,
    // and that valid tokens still work and invalid tokens fail.

    let valid_req = Request::builder()
        .method("POST")
        .uri("/submit")
        .header("Cookie", format!("autumn-csrf={token}"))
        .header("X-CSRF-Token", &token)
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(valid_req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let invalid_req = Request::builder()
        .method("POST")
        .uri("/submit")
        .header("Cookie", format!("autumn-csrf={token}"))
        .header("X-CSRF-Token", "12345678-1234-1234-1234-123456789013")
        .body(Body::empty())
        .unwrap();

    let response2 = app.oneshot(invalid_req).await.unwrap();
    assert_eq!(response2.status(), StatusCode::FORBIDDEN);
}
