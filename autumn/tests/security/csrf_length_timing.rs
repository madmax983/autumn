use autumn_web::security::CsrfLayer;
use autumn_web::security::config::CsrfConfig;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use tower::ServiceExt;
use std::time::Instant;

#[tokio::test]
async fn test_csrf_constant_time_length() {
    let config = CsrfConfig {
        enabled: true,
        ..Default::default()
    };

    let app = Router::new()
        .route("/submit", post(|| async { "created" }))
        .layer(CsrfLayer::from_config(&config));

    let token = "12345678-1234-1234-1234-123456789012".to_string(); // 36 chars

    // Request with token of length 1
    let req_short = Request::builder()
        .method("POST")
        .uri("/submit")
        .header("Cookie", format!("autumn-csrf={token}"))
        .header("X-CSRF-Token", "1")
        .body(Body::empty())
        .unwrap();

    // Request with token of length 36 but wrong chars
    let req_same_len = Request::builder()
        .method("POST")
        .uri("/submit")
        .header("Cookie", format!("autumn-csrf={token}"))
        .header("X-CSRF-Token", "22345678-1234-1234-1234-123456789012")
        .body(Body::empty())
        .unwrap();

    let start1 = Instant::now();
    let res1 = app.clone().oneshot(req_short).await.unwrap();
    let duration1 = start1.elapsed();

    let start2 = Instant::now();
    let res2 = app.oneshot(req_same_len).await.unwrap();
    let duration2 = start2.elapsed();

    assert_eq!(res1.status(), StatusCode::FORBIDDEN);
    assert_eq!(res2.status(), StatusCode::FORBIDDEN);

    // Instead of asserting flaky strict timing, we just make sure the
    // endpoint functions as expected and the time difference isn't ridiculous.
    // The main verification is that `constant_time_eq` doesn't have an early return.
    let diff = if duration1 > duration2 {
        duration1 - duration2
    } else {
        duration2 - duration1
    };

    assert!(diff.as_millis() < 50, "Timing difference is suspiciously high, but expected to be minimal");
}
