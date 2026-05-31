use autumn_web::security::{RateLimitConfig, RateLimitLayer};
use axum::{body::Body, http::Request, routing::get, Router};
use std::collections::BTreeMap;
use tower::ServiceExt;

#[tokio::test]
async fn rate_limit_bypass_via_path_normalization() {
    let mut path_overrides = BTreeMap::new();
    // Exemption / generous rate limit for /static/
    path_overrides.insert(
        "/static/".to_string(),
        autumn_web::security::RateLimitOverride {
            requests_per_second: Some(100.0),
            burst: Some(100),
        },
    );

    let config = RateLimitConfig {
        enabled: true,
        requests_per_second: 0.1, // Strict default
        burst: 1,
        trust_forwarded_headers: false,
        path_overrides,
    };

    let app = Router::new()
        .route(
            "/{*path}",
            get(
                |axum::extract::Path(path): axum::extract::Path<String>| async move {
                    format!("path: {}", path)
                },
            ),
        )
        .layer(RateLimitLayer::from_config(&config));

    // First request should pass the strict rate limit
    let req = Request::builder()
        .uri("/protected")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);

    // Second request to /protected should fail because of burst=1
    let req2 = Request::builder()
        .uri("/protected")
        .body(Body::empty())
        .unwrap();
    let res2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);

    // Bypassing by using /static/../protected should ALSO fail because the
    // logical path is /protected, which hits the strict rate limit bucket
    // (Wait, actually if it hit the /protected bucket, it would be rate limited.
    // If it incorrectly bypassed, it would hit the /static/ bucket and succeed.)
    let req_bypass = Request::builder()
        .uri("/static/../protected")
        .body(Body::empty())
        .unwrap();
    let res_bypass = app.clone().oneshot(req_bypass).await.unwrap();
    assert_eq!(
        res_bypass.status(),
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "RateLimit middleware should use the normalized path for path overrides"
    );
}
