use autumn_web::security::CsrfConfig;
use autumn_web::security::CsrfLayer;
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

    // For testing timing effectively, we need a large token payload
    // to magnify the difference between O(1) early-exit and O(N) constant-time checking
    let mut large_token = String::new();
    for _ in 0..100_000 {
        large_token.push_str("12345678-1234-1234-1234-123456789012");
    }

    // Warm up the framework
    let req_warmup = Request::builder()
        .method("POST")
        .uri("/submit")
        .header("Cookie", format!("autumn-csrf={large_token}"))
        .header("X-CSRF-Token", "warmup")
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req_warmup).await.unwrap();

    // Request with token of length 1 (short circuit timing attack vector)
    let req_short = Request::builder()
        .method("POST")
        .uri("/submit")
        .header("Cookie", format!("autumn-csrf={large_token}"))
        .header("X-CSRF-Token", "1")
        .body(Body::empty())
        .unwrap();

    // Request with token of length same but wrong chars
    let req_same_len = Request::builder()
        .method("POST")
        .uri("/submit")
        .header("Cookie", format!("autumn-csrf={large_token}"))
        .header("X-CSRF-Token", large_token.replace("1", "2"))
        .body(Body::empty())
        .unwrap();

    let start = Instant::now();
    let res1 = app.clone().oneshot(req_short).await.unwrap();
    let elapsed_short = start.elapsed();

    let start = Instant::now();
    let res2 = app.oneshot(req_same_len).await.unwrap();
    let elapsed_same = start.elapsed();

    // Both a wrong-length token and a same-length wrong token must be rejected.
    assert_eq!(res1.status(), StatusCode::FORBIDDEN);
    assert_eq!(res2.status(), StatusCode::FORBIDDEN);

    println!("Short token time: {:?}", elapsed_short);
    println!("Same length wrong time: {:?}", elapsed_same);

    // Assert that the short token is not immediately rejected faster than processing the same length wrong token.
    // subtle::ConstantTimeEq's ct_eq on slices will short-circuit on length mismatch, returning instantly.
    // By enforcing our own loop that always runs `a.len()` iterations, both should take approximately the same time.
    // Since elapsed_same can fluctuate, we just ensure that elapsed_short isn't an order of magnitude faster (as a short circuit would be).
    assert!(elapsed_short.as_millis() >= elapsed_same.as_millis() / 5, "Length mismatch short-circuit detected! Short token time: {:?}, Same length wrong time: {:?}", elapsed_short, elapsed_same);
}
