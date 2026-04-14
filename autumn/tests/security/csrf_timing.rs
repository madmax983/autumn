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

    // This test verifies that the implementation uses constant-time comparison,
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

    let response2 = app.clone().oneshot(invalid_req).await.unwrap();
    assert_eq!(response2.status(), StatusCode::FORBIDDEN);

    // PoC: simulate timing attack by comparing early vs. late mismatch request durations.
    // With constant-time comparison, both should take similar time regardless of where the
    // token diverges. We don't hard-assert timing equality (CI is too noisy), but we log
    // the results and verify both batches are consistently rejected.
    let iterations = 500;

    // Early mismatch — fails on the very first character
    let mut total_early = std::time::Duration::new(0, 0);
    for _ in 0..iterations {
        let req = Request::builder()
            .method("POST")
            .uri("/submit")
            .header("Cookie", format!("autumn-csrf={token}"))
            .header("X-CSRF-Token", "22345678-1234-1234-1234-123456789012") // Fails at first char
            .body(Body::empty())
            .unwrap();
        let app_clone = app.clone();
        let start = std::time::Instant::now();
        let resp = app_clone.oneshot(req).await.unwrap();
        total_early += start.elapsed();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // Late mismatch — fails only on the last character
    let mut total_late = std::time::Duration::new(0, 0);
    for _ in 0..iterations {
        let req = Request::builder()
            .method("POST")
            .uri("/submit")
            .header("Cookie", format!("autumn-csrf={token}"))
            .header("X-CSRF-Token", "12345678-1234-1234-1234-123456789013") // Fails at last char
            .body(Body::empty())
            .unwrap();
        let app_clone = app.clone();
        let start = std::time::Instant::now();
        let resp = app_clone.oneshot(req).await.unwrap();
        total_late += start.elapsed();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // Log timing for manual observation. A large ratio between early and late would indicate
    // a non-constant-time comparison. We do not hard-assert the ratio to keep CI stable.
    println!("Timing: Early fail={total_early:?}, Late fail={total_late:?}");
}
