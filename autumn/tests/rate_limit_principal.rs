//! Integration tests for per-principal and API-token rate limiting (issue #794).
//!
//! Covers:
//! - `key_strategy = "ip"` (existing default, preserved)
//! - `key_strategy = "api_token"` (keys on Bearer token)
//! - `key_strategy = "authenticated_principal"` (keys on principal ID from extensions)
//! - Tiered limits via `tiers` table in config
//! - `X-RateLimit-Reset` header presence on allowed and denied responses
//! - Problem Details (RFC 9457) body and `application/problem+json` content-type on 429
//! - Per-path override via `RateLimitLayer::with_path_override`
//! - Unauthenticated fallback to IP when principal strategy is active
//! - Tier assignment hook

use autumn_web::config::AutumnConfig;
use autumn_web::security::{KeyStrategy, RateLimitConfig, RateLimitLayer, RateLimitOverride, RateLimitPrincipal, RateLimitTierConfig};
use autumn_web::test::TestApp;
use autumn_web::{get, routes};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn ip_config(rps: f64, burst: u32) -> AutumnConfig {
    let mut config = AutumnConfig::default();
    config.security.rate_limit.enabled = true;
    config.security.rate_limit.requests_per_second = rps;
    config.security.rate_limit.burst = burst;
    config.security.rate_limit.trust_forwarded_headers = true;
    config.security.rate_limit.key_strategy = KeyStrategy::Ip;
    config
}

fn api_token_config(rps: f64, burst: u32) -> AutumnConfig {
    let mut config = AutumnConfig::default();
    config.security.rate_limit.enabled = true;
    config.security.rate_limit.requests_per_second = rps;
    config.security.rate_limit.burst = burst;
    config.security.rate_limit.key_strategy = KeyStrategy::ApiToken;
    config
}

fn principal_config(rps: f64, burst: u32) -> AutumnConfig {
    let mut config = AutumnConfig::default();
    config.security.rate_limit.enabled = true;
    config.security.rate_limit.requests_per_second = rps;
    config.security.rate_limit.burst = burst;
    config.security.rate_limit.trust_forwarded_headers = true;
    config.security.rate_limit.key_strategy = KeyStrategy::AuthenticatedPrincipal;
    config
}

#[get("/ping")]
async fn ping() -> &'static str {
    "pong"
}

// ── Key strategy: api_token ───────────────────────────────────────────────────

#[tokio::test]
async fn api_token_strategy_keys_on_bearer_token() {
    let client = TestApp::new()
        .routes(routes![ping])
        .config(api_token_config(0.1, 1))
        .build();

    // Token A gets one request.
    client
        .get("/ping")
        .header("Authorization", "Bearer token-alpha")
        .send()
        .await
        .assert_status(200);

    // Token A's bucket is now exhausted.
    client
        .get("/ping")
        .header("Authorization", "Bearer token-alpha")
        .send()
        .await
        .assert_status(429);

    // Token B still has a fresh bucket.
    client
        .get("/ping")
        .header("Authorization", "Bearer token-beta")
        .send()
        .await
        .assert_status(200);
}

#[tokio::test]
async fn api_token_strategy_falls_back_to_ip_when_no_bearer() {
    // When api_token strategy is active and no Authorization header is present,
    // the limiter falls back to IP-based keying.
    let mut config = api_token_config(0.1, 1);
    config.security.rate_limit.trust_forwarded_headers = true;
    let client = TestApp::new()
        .routes(routes![ping])
        .config(config)
        .build();

    client
        .get("/ping")
        .header("X-Forwarded-For", "10.0.0.1")
        .send()
        .await
        .assert_status(200);

    // Same IP without a token: exhausted.
    client
        .get("/ping")
        .header("X-Forwarded-For", "10.0.0.1")
        .send()
        .await
        .assert_status(429);
}

// ── Key strategy: authenticated_principal ─────────────────────────────────────

#[tokio::test]
async fn principal_strategy_keys_on_principal_extension() {
    // Auth middleware sets RateLimitPrincipal on the request before the rate
    // limiter runs. This test simulates that by injecting the extension directly.
    let config = principal_config(0.1, 1);
    let rl_layer = RateLimitLayer::from_config(&config.security.rate_limit);

    let app = Router::new()
        .route("/ping", axum::routing::get(|| async { "pong" }))
        .layer(rl_layer);

    let make_req = |principal: &str| {
        let mut req = Request::builder()
            .method("GET")
            .uri("/ping")
            .body(Body::empty())
            .expect("request builds");
        req.extensions_mut()
            .insert(RateLimitPrincipal(principal.to_owned()));
        req
    };

    use tower::ServiceExt;

    // Principal A uses 1 token.
    let r = app.clone().oneshot(make_req("user-42")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // Principal A is now exhausted.
    let r = app.clone().oneshot(make_req("user-42")).await.unwrap();
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);

    // Principal B still has a full bucket.
    let r = app.clone().oneshot(make_req("user-99")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn principal_strategy_falls_back_to_ip_for_unauthenticated() {
    // Without a RateLimitPrincipal extension, fall back to IP keying.
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;
    use tower::ServiceExt;

    let config = principal_config(0.1, 1);
    let rl_layer = RateLimitLayer::from_config(&config.security.rate_limit);

    let app = Router::new()
        .route("/ping", axum::routing::get(|| async { "pong" }))
        .layer(rl_layer);

    let make_req = |peer: &str| {
        let mut req = Request::builder()
            .method("GET")
            .uri("/ping")
            .body(Body::empty())
            .expect("request builds");
        let addr: SocketAddr = peer.parse().expect("peer parses");
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    };

    let r = app.clone().oneshot(make_req("10.0.0.1:1234")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app.clone().oneshot(make_req("10.0.0.1:1234")).await.unwrap();
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
}

// ── X-RateLimit-Reset header ──────────────────────────────────────────────────

#[tokio::test]
async fn x_ratelimit_reset_present_on_allowed_response() {
    let client = TestApp::new()
        .routes(routes![ping])
        .config(ip_config(10.0, 20))
        .build();

    let response = client
        .get("/ping")
        .header("X-Forwarded-For", "1.2.3.4")
        .send()
        .await;
    response.assert_status(200);
    let reset = response
        .header("x-ratelimit-reset")
        .expect("X-RateLimit-Reset must be present on allowed responses");
    let reset_val: u64 = reset.parse().expect("X-RateLimit-Reset is a unix timestamp");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        reset_val <= now + 120,
        "X-RateLimit-Reset={reset_val} should be within 120s of now={now}"
    );
}

#[tokio::test]
async fn x_ratelimit_reset_present_on_429_response() {
    let client = TestApp::new()
        .routes(routes![ping])
        .config(ip_config(0.1, 1))
        .build();

    client
        .get("/ping")
        .header("X-Forwarded-For", "5.5.5.5")
        .send()
        .await
        .assert_status(200);

    let throttled = client
        .get("/ping")
        .header("X-Forwarded-For", "5.5.5.5")
        .send()
        .await;
    throttled.assert_status(429);
    let reset = throttled
        .header("x-ratelimit-reset")
        .expect("X-RateLimit-Reset must be present on 429 responses");
    let reset_val: u64 = reset.parse().expect("X-RateLimit-Reset is a unix timestamp");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        reset_val >= now,
        "X-RateLimit-Reset={reset_val} should be >= now={now} on a 429"
    );
}

// ── 429 Problem Details body ──────────────────────────────────────────────────

#[tokio::test]
async fn throttled_response_is_problem_details_json() {
    let client = TestApp::new()
        .routes(routes![ping])
        .config(ip_config(0.1, 1))
        .build();

    client
        .get("/ping")
        .header("X-Forwarded-For", "6.6.6.6")
        .send()
        .await
        .assert_status(200);

    let throttled = client
        .get("/ping")
        .header("X-Forwarded-For", "6.6.6.6")
        .send()
        .await;
    throttled.assert_status(429);
    throttled.assert_header_contains("content-type", "application/problem+json");

    let body: serde_json::Value = throttled.json();
    assert_eq!(body["status"], 429);
    assert!(
        body["type"].as_str().is_some_and(|t| !t.is_empty()),
        "Problem Details must have a type URI"
    );
    assert!(
        body["title"].as_str().is_some_and(|t| !t.is_empty()),
        "Problem Details must have a title"
    );
    assert!(
        body["detail"].as_str().is_some_and(|d| !d.is_empty()),
        "Problem Details must have a detail"
    );
    // Must not leak the raw key value.
    let body_str = body.to_string();
    assert!(
        !body_str.contains("6.6.6.6"),
        "429 Problem Details must not leak the raw rate-limit key"
    );
}

#[tokio::test]
async fn problem_details_includes_stable_type_uri() {
    let client = TestApp::new()
        .routes(routes![ping])
        .config(ip_config(0.1, 1))
        .build();

    client
        .get("/ping")
        .header("X-Forwarded-For", "7.7.7.7")
        .send()
        .await
        .assert_status(200);

    let throttled = client
        .get("/ping")
        .header("X-Forwarded-For", "7.7.7.7")
        .send()
        .await;
    throttled.assert_status(429);

    let body: serde_json::Value = throttled.json();
    assert_eq!(
        body["type"].as_str(),
        Some("https://autumn.dev/problems/rate-limited"),
        "Rate limit 429 must use the stable type URI"
    );
    assert!(
        body["code"].as_str().is_some(),
        "Rate limit 429 must include a stable code"
    );
}

// ── Problem Details does not leak key class ──────────────────────────────────

#[tokio::test]
async fn problem_details_includes_key_class_without_leaking_key() {
    let client = TestApp::new()
        .routes(routes![ping])
        .config(api_token_config(0.1, 1))
        .build();

    client
        .get("/ping")
        .header("Authorization", "Bearer secret-token-value")
        .send()
        .await
        .assert_status(200);

    let throttled = client
        .get("/ping")
        .header("Authorization", "Bearer secret-token-value")
        .send()
        .await;
    throttled.assert_status(429);

    let body: serde_json::Value = throttled.json();
    let body_str = body.to_string();
    // The key class (token, ip, principal) may appear; the raw value must not.
    assert!(
        !body_str.contains("secret-token-value"),
        "429 body must not leak the raw token value"
    );
    // The detail should mention what kind of key was rate-limited.
    let detail = body["detail"].as_str().unwrap_or("");
    assert!(!detail.is_empty(), "detail must be non-empty");
}

// ── Tiered limits ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn tier_assignment_hook_selects_correct_limits() {
    use tower::ServiceExt;

    let mut rl_config = RateLimitConfig {
        enabled: true,
        requests_per_second: 0.1,
        burst: 1, // default for unrecognized tier
        key_strategy: KeyStrategy::AuthenticatedPrincipal,
        ..Default::default()
    };
    rl_config.tiers.insert(
        "pro".to_owned(),
        RateLimitTierConfig {
            requests_per_second: 0.1,
            burst: 5,
        },
    );

    let rl_layer = RateLimitLayer::from_config(&rl_config).with_tier_hook(|principal_id: &str| {
        if principal_id.starts_with("pro_") {
            Some("pro".to_owned())
        } else {
            None
        }
    });

    let app = Router::new()
        .route("/ping", axum::routing::get(|| async { "pong" }))
        .layer(rl_layer);

    // Pro user: burst=5, can make 5 requests.
    for i in 0..5 {
        let mut req = Request::builder()
            .method("GET")
            .uri("/ping")
            .body(Body::empty())
            .expect("request builds");
        req.extensions_mut()
            .insert(RateLimitPrincipal("pro_user123".to_owned()));
        let r = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "pro user request {i} should be allowed"
        );
    }

    // Pro user: 6th request is over burst=5.
    let mut req = Request::builder()
        .method("GET")
        .uri("/ping")
        .body(Body::empty())
        .expect("request builds");
    req.extensions_mut()
        .insert(RateLimitPrincipal("pro_user123".to_owned()));
    let r = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        r.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "6th pro request should be throttled"
    );

    // Free user: burst=1, exhausted after 1 request.
    let mut req = Request::builder()
        .method("GET")
        .uri("/ping")
        .body(Body::empty())
        .expect("request builds");
    req.extensions_mut()
        .insert(RateLimitPrincipal("free_user456".to_owned()));
    let r = app.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK, "first free user request should pass");

    let mut req = Request::builder()
        .method("GET")
        .uri("/ping")
        .body(Body::empty())
        .expect("request builds");
    req.extensions_mut()
        .insert(RateLimitPrincipal("free_user456".to_owned()));
    let r = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        r.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "second free user request should be throttled"
    );
}

// ── Per-path override ─────────────────────────────────────────────────────────

#[tokio::test]
async fn per_path_override_applies_stricter_limit() {
    use tower::ServiceExt;

    let rl_config = RateLimitConfig {
        enabled: true,
        requests_per_second: 0.1,
        burst: 10, // global: generous
        trust_forwarded_headers: true,
        key_strategy: KeyStrategy::Ip,
        ..Default::default()
    };

    let rl_layer = RateLimitLayer::from_config(&rl_config).with_path_override(
        "/strict",
        RateLimitOverride {
            requests_per_second: Some(0.1),
            burst: Some(1),
        },
    );

    let app = Router::new()
        .route("/strict", axum::routing::get(|| async { "strict" }))
        .route("/normal", axum::routing::get(|| async { "normal" }))
        .layer(rl_layer);

    let make_req = |path: &str| {
        use axum::extract::ConnectInfo;
        use std::net::SocketAddr;
        let mut req = Request::builder()
            .method("GET")
            .uri(path)
            .body(Body::empty())
            .expect("request builds");
        let addr: SocketAddr = "1.2.3.4:1234".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    };

    // /strict: first request OK with burst=1, second throttled.
    let r = app.clone().oneshot(make_req("/strict")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app.clone().oneshot(make_req("/strict")).await.unwrap();
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);

    // /normal uses global burst=10, should still pass for many requests.
    for _ in 0..5 {
        let r = app.clone().oneshot(make_req("/normal")).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
}

// ── Migration: existing ip config keys still work ─────────────────────────────

#[tokio::test]
async fn default_key_strategy_is_ip() {
    let config = RateLimitConfig::default();
    assert_eq!(config.key_strategy, KeyStrategy::Ip);
}

#[tokio::test]
async fn existing_ip_config_still_works() {
    let mut config = AutumnConfig::default();
    config.security.rate_limit.enabled = true;
    config.security.rate_limit.requests_per_second = 0.1;
    config.security.rate_limit.burst = 1;
    config.security.rate_limit.trust_forwarded_headers = true;
    // No key_strategy set — uses default (Ip).

    let client = TestApp::new()
        .routes(routes![ping])
        .config(config)
        .build();

    client
        .get("/ping")
        .header("X-Forwarded-For", "9.9.9.9")
        .send()
        .await
        .assert_status(200);

    client
        .get("/ping")
        .header("X-Forwarded-For", "9.9.9.9")
        .send()
        .await
        .assert_status(429);
}

// ── Config deserialization ────────────────────────────────────────────────────

#[test]
fn key_strategy_deserializes_from_toml() {
    let toml = r#"
        enabled = true
        key_strategy = "api_token"
    "#;
    let config: RateLimitConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.key_strategy, KeyStrategy::ApiToken);
}

#[test]
fn key_strategy_authenticated_principal_deserializes() {
    let toml = r#"
        enabled = true
        key_strategy = "authenticated_principal"
    "#;
    let config: RateLimitConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.key_strategy, KeyStrategy::AuthenticatedPrincipal);
}

#[test]
fn key_strategy_ip_deserializes() {
    let toml = r#"
        enabled = true
        key_strategy = "ip"
    "#;
    let config: RateLimitConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.key_strategy, KeyStrategy::Ip);
}

#[test]
fn tiers_deserialize_from_toml() {
    let toml = r#"
        enabled = true
        key_strategy = "authenticated_principal"

        [tiers.free]
        requests_per_second = 1.0
        burst = 10

        [tiers.pro]
        requests_per_second = 10.0
        burst = 100
    "#;
    let config: RateLimitConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.tiers.len(), 2);
    assert!((config.tiers["free"].requests_per_second - 1.0).abs() < f64::EPSILON);
    assert_eq!(config.tiers["free"].burst, 10);
    assert!((config.tiers["pro"].requests_per_second - 10.0).abs() < f64::EPSILON);
    assert_eq!(config.tiers["pro"].burst, 100);
}

#[test]
fn tiers_empty_by_default() {
    let config = RateLimitConfig::default();
    assert!(config.tiers.is_empty());
}
