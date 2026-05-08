use autumn_web::config::AutumnConfig;
use autumn_web::test::TestApp;
use autumn_web::{get, routes};

#[get("/test")]
async fn test_handler() -> &'static str {
    "test"
}

#[tokio::test]
async fn eris_rate_limit_bypass_poc() {
    let mut config = AutumnConfig::default();
    config.security.rate_limit.enabled = true;
    config.security.rate_limit.requests_per_second = 1.0;
    config.security.rate_limit.burst = 1;
    config.security.rate_limit.trust_forwarded_headers = true;

    let client = TestApp::new()
        .config(config)
        .routes(routes![test_handler])
        .build();

    // First request, consumes burst
    client
        .get("/test")
        .header("X-Forwarded-For", "fake_ip_1, real_ip")
        .send()
        .await
        .assert_status(200);

    // Second request, should be rate limited if using real_ip.
    // If it uses fake_ip_2, it gets a fresh bucket and bypasses the limit!
    let throttled = client
        .get("/test")
        .header("X-Forwarded-For", "fake_ip_2, real_ip")
        .send()
        .await;

    // We expect 429 TOO MANY REQUESTS if fixed.
    assert_eq!(
        throttled.status, 429,
        "VULNERABILITY: Rate limit bypassed via X-Forwarded-For spoofing!"
    );
}
