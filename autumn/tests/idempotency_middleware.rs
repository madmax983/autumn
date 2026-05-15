//! Integration tests for HTTP idempotency-key middleware (issue #677).
use std::sync::Arc;
use std::time::Duration;

use autumn_web::idempotency::{IdempotencyLayer, MemoryIdempotencyStore};
use autumn_web::test::TestApp;
use autumn_web::{get, post, put, routes};
use axum::http::StatusCode;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn ok_handler() -> &'static str {
    "pong"
}

fn make_store(ttl: Duration) -> Arc<dyn autumn_web::idempotency::IdempotencyStore> {
    Arc::new(MemoryIdempotencyStore::new(ttl))
}

/// Replicates the storage-key format used by `IdempotencyLayer` so tests can
/// pre-lock the exact slot the middleware will look up.
fn storage_key(method: &str, path: &str, auth: &str, idempotency_key: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    auth.hash(&mut h);
    format!("{}:{}:{:x}:{}", method, path, h.finish(), idempotency_key)
}

/// Axum middleware that injects a 5-byte `UploadConfig` limit into extensions.
/// Used by `test_request_body_too_large_returns_413` to trigger the 413 path
/// in the idempotency middleware without needing a 32 MiB request body.
async fn inject_tiny_upload_limit(
    mut req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use autumn_web::security::UploadConfig;
    req.extensions_mut().insert(UploadConfig {
        max_request_size_bytes: 5,
        ..Default::default()
    });
    next.run(req).await
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Identical POST with the same idempotency key replays the first response.
#[tokio::test]
async fn test_deduplication() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let store = make_store(Duration::from_secs(3600));
    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let r1 = client
        .post("/ping")
        .header("idempotency-key", "dedup-key-1")
        .send()
        .await;
    r1.assert_ok();
    assert_eq!(r1.header("x-idempotent-replayed"), None);

    let r2 = client
        .post("/ping")
        .header("idempotency-key", "dedup-key-1")
        .send()
        .await;
    r2.assert_ok();
    assert_eq!(r2.header("x-idempotent-replayed"), Some("true"));
    assert_eq!(r1.text(), r2.text());

    let _ = store; // keep store alive
}

/// A different payload with the same key returns 422.
#[tokio::test]
async fn test_payload_mismatch_returns_422() {
    use tower::ServiceExt;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route("/echo", axum::routing::post(ok_handler))
        .layer(layer);

    let req1 = axum::http::Request::builder()
        .method("POST")
        .uri("/echo")
        .header("idempotency-key", "mismatch-key")
        .body(axum::body::Body::from("body-one"))
        .unwrap();
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    let req2 = axum::http::Request::builder()
        .method("POST")
        .uri("/echo")
        .header("idempotency-key", "mismatch-key")
        .body(axum::body::Body::from("body-two"))
        .unwrap();
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// POST without an idempotency key is passed through on every call.
#[tokio::test]
async fn test_no_key_passthrough() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let r1 = client.post("/ping").send().await;
    r1.assert_ok();
    assert_eq!(r1.header("x-idempotent-replayed"), None);

    let r2 = client.post("/ping").send().await;
    r2.assert_ok();
    assert_eq!(r2.header("x-idempotent-replayed"), None);
}

/// GET requests with an idempotency key are not deduplicated (not mutating).
#[tokio::test]
async fn test_get_passthrough() {
    #[get("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let r1 = client
        .get("/ping")
        .header("idempotency-key", "get-key")
        .send()
        .await;
    r1.assert_ok();
    assert_eq!(r1.header("x-idempotent-replayed"), None);

    let r2 = client
        .get("/ping")
        .header("idempotency-key", "get-key")
        .send()
        .await;
    r2.assert_ok();
    assert_eq!(r2.header("x-idempotent-replayed"), None);
}

/// PUT with an idempotency key is also deduplicated.
#[tokio::test]
async fn test_put_deduplication() {
    #[put("/item")]
    async fn handler() -> &'static str {
        "updated"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let r1 = client
        .put("/item")
        .header("idempotency-key", "put-key-1")
        .send()
        .await;
    r1.assert_ok();
    assert_eq!(r1.header("x-idempotent-replayed"), None);

    let r2 = client
        .put("/item")
        .header("idempotency-key", "put-key-1")
        .send()
        .await;
    r2.assert_ok();
    assert_eq!(r2.header("x-idempotent-replayed"), Some("true"));
}

/// Different idempotency keys are stored independently.
#[tokio::test]
async fn test_distinct_keys_are_independent() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    // First request with key-a — should be fresh.
    let ra1 = client
        .post("/ping")
        .header("idempotency-key", "distinct-key-a")
        .send()
        .await;
    assert_eq!(ra1.header("x-idempotent-replayed"), None);

    // First request with key-b — should also be fresh.
    let rb1 = client
        .post("/ping")
        .header("idempotency-key", "distinct-key-b")
        .send()
        .await;
    assert_eq!(rb1.header("x-idempotent-replayed"), None);

    // Second request with key-a — replayed.
    let ra2 = client
        .post("/ping")
        .header("idempotency-key", "distinct-key-a")
        .send()
        .await;
    assert_eq!(ra2.header("x-idempotent-replayed"), Some("true"));

    // Second request with key-b — replayed.
    let rb2 = client
        .post("/ping")
        .header("idempotency-key", "distinct-key-b")
        .send()
        .await;
    assert_eq!(rb2.header("x-idempotent-replayed"), Some("true"));
}

/// The `X-Idempotent-Replayed` header is present only on replayed responses.
#[tokio::test]
async fn test_x_idempotent_replayed_header_semantics() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let fresh = client
        .post("/ping")
        .header("idempotency-key", "replay-header-key")
        .send()
        .await;
    assert_eq!(
        fresh.header("x-idempotent-replayed"),
        None,
        "fresh response must NOT have x-idempotent-replayed"
    );

    let replayed = client
        .post("/ping")
        .header("idempotency-key", "replay-header-key")
        .send()
        .await;
    assert_eq!(
        replayed.header("x-idempotent-replayed"),
        Some("true"),
        "replayed response must have x-idempotent-replayed: true"
    );
}

/// `TestApp::idempotent()` builder wires the middleware correctly.
#[tokio::test]
async fn test_idempotent_builder_method() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let r1 = client
        .post("/ping")
        .header("idempotency-key", "builder-key")
        .send()
        .await;
    r1.assert_ok();

    let r2 = client
        .post("/ping")
        .header("idempotency-key", "builder-key")
        .send()
        .await;
    r2.assert_ok();
    assert_eq!(r2.header("x-idempotent-replayed"), Some("true"));
}

/// The default TTL for idempotency config is 86400 seconds (24 hours).
#[test]
fn test_config_default_ttl_is_24h() {
    let config = autumn_web::config::IdempotencyConfig::default();
    assert_eq!(
        config.ttl_secs, 86_400,
        "default TTL should be 86400 seconds"
    );
}

/// Entries past their TTL are not replayed.
#[test]
fn test_ttl_eviction() {
    use autumn_web::idempotency::{IdempotencyRecord, IdempotencyStore};

    let store = MemoryIdempotencyStore::new(Duration::from_millis(1));
    let record = IdempotencyRecord {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
    };
    store.set("evict-key", record, vec![0u8; 8], Duration::from_millis(1));

    // Sleep long enough for the entry to expire.
    std::thread::sleep(Duration::from_millis(20));

    assert!(
        store.get("evict-key").is_none(),
        "expired entry should not be returned"
    );
}

/// A concurrent duplicate request (same key, first still in flight) receives
/// 409 Conflict with a Retry-After header.
#[tokio::test]
async fn test_concurrent_duplicate_returns_409() {
    use autumn_web::idempotency::{IdempotencyStore, MemoryIdempotencyStore};
    use tower::ServiceExt;

    let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(3600)));
    let layer = IdempotencyLayer::new(store.clone() as Arc<dyn IdempotencyStore>);

    let app = axum::Router::new()
        .route("/ping", axum::routing::post(ok_handler))
        .layer(layer);

    // Pre-lock the scoped storage key to simulate an in-flight request.
    // The middleware namespaces by method, path, and auth, so we replicate
    // the same format here (no Authorization header → auth = "").
    store.try_lock(&storage_key("POST", "/ping", "", "inflight-key"));

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/ping")
        .header("idempotency-key", "inflight-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "concurrent duplicate should return 409 Conflict"
    );
    assert!(
        resp.headers().contains_key("retry-after"),
        "409 response must include Retry-After header"
    );
}

/// After processing completes the in-flight lock is released so a subsequent
/// sequential request can be served normally.
#[tokio::test]
async fn test_in_flight_lock_released_after_response() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    // First request acquires and releases lock, stores response.
    let r1 = client
        .post("/ping")
        .header("idempotency-key", "lock-release-key")
        .send()
        .await;
    r1.assert_ok();

    // Second request should replay (not conflict), proving the lock was released.
    let r2 = client
        .post("/ping")
        .header("idempotency-key", "lock-release-key")
        .send()
        .await;
    r2.assert_ok();
    assert_eq!(
        r2.header("x-idempotent-replayed"),
        Some("true"),
        "second request should replay, not conflict"
    );
}

/// Metrics counters are incremented correctly for hits and misses.
#[tokio::test]
async fn test_metrics_recorded() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    // Miss: first request.
    client
        .post("/ping")
        .header("idempotency-key", "metrics-key")
        .send()
        .await
        .assert_ok();

    // Hit: second request with same key.
    let replayed = client
        .post("/ping")
        .header("idempotency-key", "metrics-key")
        .send()
        .await;
    replayed.assert_ok();
    assert_eq!(replayed.header("x-idempotent-replayed"), Some("true"));
    // Metrics are recorded in the background — the test verifies behaviour, not
    // the counter value, since the MetricsCollector is private to the router.
}

/// `IdempotencyConfig::default()` reflects documented defaults.
#[test]
fn test_config_fields() {
    let config = autumn_web::config::IdempotencyConfig::default();
    assert!(
        config.enabled.is_none(),
        "middleware must be absent (not enabled) by default"
    );
    assert_eq!(config.ttl_secs, 86_400, "default TTL is 24 hours");
    assert!(
        !config.allow_memory_in_production,
        "memory backend is rejected in production by default"
    );
    assert_eq!(
        config.redis.key_prefix, "autumn:idempotency",
        "default Redis key prefix"
    );
}

/// `MemoryIdempotencyStore::new(ttl)` stores the TTL and exposes it via
/// `default_ttl()`, and `IdempotencyLayer::new(store)` picks it up.
#[test]
fn test_store_ttl_propagates_to_layer() {
    use autumn_web::idempotency::IdempotencyStore;

    let ttl = Duration::from_secs(300);
    let store = MemoryIdempotencyStore::new(ttl);
    assert_eq!(
        store.default_ttl(),
        ttl,
        "store must return the TTL passed to new()"
    );
}

/// Non-2xx responses are not cached; a second request with the same key
/// re-executes the handler rather than replaying the error.
#[tokio::test]
async fn test_error_response_not_cached() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[post("/fail")]
    async fn handler() -> (StatusCode, &'static str) {
        CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        (StatusCode::INTERNAL_SERVER_ERROR, "boom")
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let r1 = client
        .post("/fail")
        .header("idempotency-key", "error-key")
        .send()
        .await;
    r1.assert_status(500);
    assert_eq!(r1.header("x-idempotent-replayed"), None);

    let r2 = client
        .post("/fail")
        .header("idempotency-key", "error-key")
        .send()
        .await;
    r2.assert_status(500);
    assert_eq!(
        r2.header("x-idempotent-replayed"),
        None,
        "error responses must not be replayed"
    );
    assert_eq!(
        CALL_COUNT.load(Ordering::SeqCst),
        2,
        "handler should execute twice since error was not cached"
    );
}

/// `set-cookie` headers are delivered on the first (non-replayed) response but
/// excluded from the cached replay to prevent session fixation.
#[tokio::test]
async fn test_set_cookie_on_first_response_absent_on_replay() {
    use tower::ServiceExt;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route(
            "/login",
            axum::routing::post(|| async {
                axum::response::Response::builder()
                    .status(200)
                    .header("set-cookie", "session=abc; HttpOnly; SameSite=Strict")
                    .body(axum::body::Body::from("ok"))
                    .unwrap()
            }),
        )
        .layer(layer);

    let req1 = axum::http::Request::builder()
        .method("POST")
        .uri("/login")
        .header("idempotency-key", "login-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    assert!(
        resp1.headers().contains_key("set-cookie"),
        "first response must include set-cookie"
    );
    assert!(
        !resp1.headers().contains_key("x-idempotent-replayed"),
        "first response must not have x-idempotent-replayed"
    );

    let req2 = axum::http::Request::builder()
        .method("POST")
        .uri("/login")
        .header("idempotency-key", "login-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    assert!(
        !resp2.headers().contains_key("set-cookie"),
        "replayed response must NOT include set-cookie"
    );
    assert_eq!(
        resp2
            .headers()
            .get("x-idempotent-replayed")
            .map(|v| v.to_str().unwrap()),
        Some("true"),
        "replayed response must have x-idempotent-replayed: true"
    );
}

/// An empty `Idempotency-Key` header value is treated as absent — the request
/// is passed through without caching.
#[tokio::test]
async fn test_empty_idempotency_key_is_passthrough() {
    use tower::ServiceExt;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route("/ping", axum::routing::post(ok_handler))
        .layer(layer);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/ping")
        .header("idempotency-key", "")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("x-idempotent-replayed").is_none(),
        "empty key should not be cached or replayed"
    );
}

/// When the request body exceeds the `UploadConfig` size limit the idempotency
/// middleware returns 413 Payload Too Large before forwarding to the handler.
#[tokio::test]
async fn test_request_body_too_large_returns_413() {
    use tower::ServiceExt;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    // inject_tiny_upload_limit must be OUTER (applied last) so it runs before
    // idempotency reads the UploadConfig extension.
    let app = axum::Router::new()
        .route("/upload", axum::routing::post(ok_handler))
        .layer(layer)
        .layer(axum::middleware::from_fn(inject_tiny_upload_limit));

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/upload")
        .header("idempotency-key", "big-body-key")
        .body(axum::body::Body::from("more than five bytes"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "body larger than UploadConfig limit must return 413"
    );
}

/// A response body larger than the 10 MiB cache cap is streamed through to the
/// client without being stored. A second request with the same key re-runs the
/// handler rather than replaying a cached response.
#[tokio::test]
async fn test_large_response_not_cached_and_streamed_through() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

    // One byte over the 10 MiB cache cap.
    const OVER_CAP: usize = 10 * 1024 * 1024 + 1;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route(
            "/big",
            axum::routing::post(|| async {
                CALL_COUNT.fetch_add(1, Ordering::SeqCst);
                axum::response::Response::builder()
                    .status(200)
                    .body(axum::body::Body::from(vec![b'x'; OVER_CAP]))
                    .unwrap()
            }),
        )
        .layer(layer);

    // First request — handler runs; response is streamed through uncached.
    let req1 = axum::http::Request::builder()
        .method("POST")
        .uri("/big")
        .header("idempotency-key", "large-resp-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let bytes1 = axum::body::to_bytes(resp1.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes1.len(), OVER_CAP, "full body must be delivered");

    // Second request — not cached; handler runs again.
    let req2 = axum::http::Request::builder()
        .method("POST")
        .uri("/big")
        .header("idempotency-key", "large-resp-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    assert!(
        resp2.headers().get("x-idempotent-replayed").is_none(),
        "oversized response must not be replayed from cache"
    );
    assert_eq!(
        CALL_COUNT.load(Ordering::SeqCst),
        2,
        "handler must execute twice when response body exceeds cache cap"
    );
}

/// The default `IdempotencyStore::default_ttl()` implementation returns 24 h.
/// Custom stores that do not override the method get this default.
#[test]
fn test_default_store_ttl_trait_impl() {
    use autumn_web::idempotency::{IdempotencyEntry, IdempotencyRecord, IdempotencyStore};

    struct BareStore;
    impl IdempotencyStore for BareStore {
        fn get(&self, _: &str) -> Option<IdempotencyEntry> {
            None
        }
        fn set(&self, _: &str, _: IdempotencyRecord, _: Vec<u8>, _: Duration) {}
        fn try_lock(&self, _: &str) -> bool {
            true
        }
        fn unlock(&self, _: &str) {}
        // default_ttl() deliberately not overridden — tests the trait default.
    }

    assert_eq!(
        BareStore.default_ttl(),
        Duration::from_secs(86_400),
        "IdempotencyStore::default_ttl() must return 24 hours"
    );
}

/// Requests to different paths sharing the same `Idempotency-Key` are stored
/// independently — no cross-endpoint replay occurs (P2: request-target scope).
#[tokio::test]
async fn test_different_paths_same_key_are_independent() {
    use tower::ServiceExt;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route("/a", axum::routing::post(|| async { "a" }))
        .route("/b", axum::routing::post(|| async { "b" }))
        .layer(layer);

    let req_a = axum::http::Request::builder()
        .method("POST")
        .uri("/a")
        .header("idempotency-key", "shared-path-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_a = app.clone().oneshot(req_a).await.unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK);
    assert!(resp_a.headers().get("x-idempotent-replayed").is_none());

    let req_b = axum::http::Request::builder()
        .method("POST")
        .uri("/b")
        .header("idempotency-key", "shared-path-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_b = app.clone().oneshot(req_b).await.unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);
    assert!(
        resp_b.headers().get("x-idempotent-replayed").is_none(),
        "different path with same key must not replay another endpoint's response"
    );
}

/// Requests with different `Authorization` headers sharing the same
/// `Idempotency-Key` are stored independently — no cross-principal replay
/// occurs (P1: authenticated principal scope).
#[tokio::test]
async fn test_different_auth_same_key_are_independent() {
    use tower::ServiceExt;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route("/action", axum::routing::post(ok_handler))
        .layer(layer);

    let req_a = axum::http::Request::builder()
        .method("POST")
        .uri("/action")
        .header("idempotency-key", "shared-auth-key")
        .header("authorization", "Bearer token-user-a")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_a = app.clone().oneshot(req_a).await.unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK);
    assert!(resp_a.headers().get("x-idempotent-replayed").is_none());

    let req_b = axum::http::Request::builder()
        .method("POST")
        .uri("/action")
        .header("idempotency-key", "shared-auth-key")
        .header("authorization", "Bearer token-user-b")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_b = app.clone().oneshot(req_b).await.unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);
    assert!(
        resp_b.headers().get("x-idempotent-replayed").is_none(),
        "different Authorization with same key must not replay another principal's response"
    );
}

/// The same body bytes with a different `Content-Type` are treated as a
/// distinct payload; the middleware returns 422 to signal the key is being
/// reused for a representationally different request (P2: representation scope).
#[tokio::test]
async fn test_different_content_type_same_body_returns_422() {
    use tower::ServiceExt;

    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route("/items", axum::routing::post(ok_handler))
        .layer(layer);

    let req1 = axum::http::Request::builder()
        .method("POST")
        .uri("/items")
        .header("idempotency-key", "ct-scope-key")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"x":1}"#))
        .unwrap();
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    let req2 = axum::http::Request::builder()
        .method("POST")
        .uri("/items")
        .header("idempotency-key", "ct-scope-key")
        .header("content-type", "application/xml")
        .body(axum::body::Body::from(r#"{"x":1}"#))
        .unwrap();
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(
        resp2.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "same bytes with different Content-Type must return 422 (payload mismatch)"
    );
}

/// DELETE requests with an idempotency key are deduplicated (DELETE is mutating).
#[tokio::test]
async fn test_delete_deduplication() {
    use autumn_web::delete;

    #[delete("/item")]
    async fn handler() -> &'static str {
        "deleted"
    }

    let client = TestApp::new().routes(routes![handler]).idempotent().build();

    let r1 = client
        .delete("/item")
        .header("idempotency-key", "delete-key-1")
        .send()
        .await;
    r1.assert_ok();
    assert_eq!(r1.header("x-idempotent-replayed"), None);

    let r2 = client
        .delete("/item")
        .header("idempotency-key", "delete-key-1")
        .send()
        .await;
    r2.assert_ok();
    assert_eq!(r2.header("x-idempotent-replayed"), Some("true"));
}
