use autumn_web::config::AutumnConfig;
use autumn_web::test::TestApp;
use axum::http::StatusCode;

#[tokio::test]
#[allow(clippy::similar_names)]
async fn test_fallback_middleware_bypass() {
    let mut config = AutumnConfig::default();
    config.security.rate_limit.enabled = true;
    config.security.rate_limit.requests_per_second = 0.0001; // very strict
    config.security.rate_limit.burst = 1;
    // Trust forwarded headers to allow IP spoofing for the test
    config.security.rate_limit.trust_forwarded_headers = true;

    // Use TestApp to build the application router, verifying Autumn's actual middleware assembly order.
    let client = TestApp::new().config(config).build();

    let req_one = client
        .get("/not-found")
        .header("X-Forwarded-For", "198.51.100.1");

    let resp_one = req_one.send().await;
    assert_eq!(resp_one.status, StatusCode::NOT_FOUND);

    let req_two = client
        .get("/not-found")
        .header("X-Forwarded-For", "198.51.100.1");

    let resp_two = req_two.send().await;

    // The second request should be rate limited if the fallback is correctly protected.
    assert_eq!(
        resp_two.status,
        StatusCode::TOO_MANY_REQUESTS,
        "VULNERABILITY: Fallback bypassed rate limit!"
    );
}
