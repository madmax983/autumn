use autumn_web::config::AutumnConfig;
use autumn_web::test::TestApp;
use autumn_web::{get, routes};

#[get("/test")]
async fn test_handler() -> &'static str {
    "test"
}

#[tokio::test]
async fn eris_fallback_middleware_bypass_poc() {
    let mut config = AutumnConfig::default();
    config.security.rate_limit.enabled = true;
    config.security.rate_limit.requests_per_second = 1.0;
    config.security.rate_limit.burst = 1;
    config.security.rate_limit.trust_forwarded_headers = true;

    // Use TestApp to build the application and its router
    let client = TestApp::new()
        .config(config)
        .routes(routes![test_handler])
        .build();

    // The first request will succeed and consume the burst capacity.
    client
        .get("/does-not-exist")
        .header("X-Forwarded-For", "127.0.0.1")
        .send()
        .await
        .assert_status(404);

    // The second request should be blocked by rate limit, returning 429.
    // If the vulnerability exists (fallback route bypasses middleware), it will return 404 again.
    let throttled = client
        .get("/does-not-exist")
        .header("X-Forwarded-For", "127.0.0.1")
        .send()
        .await;

    let status = throttled.status;

    // We expect 429 TOO MANY REQUESTS if fixed
    assert_eq!(
        status, 429,
        "Fallback bypasses rate limit middleware - returned {status}"
    );
}
