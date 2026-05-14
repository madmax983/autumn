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

// ── tests ─────────────────────────────────────────────────────────────────────

/// Identical POST with the same idempotency key replays the first response.
#[tokio::test]
async fn test_deduplication() {
    #[post("/ping")]
    async fn handler() -> &'static str {
        "pong"
    }

    let store = make_store(Duration::from_secs(3600));
    let client = TestApp::new()
        .routes(routes![handler])
        .idempotent()
        .build();

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
    let store = make_store(Duration::from_secs(3600));
    let layer = IdempotencyLayer::new(store);

    let app = axum::Router::new()
        .route("/echo", axum::routing::post(ok_handler))
        .layer(layer);

    use tower::ServiceExt;

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

    let client = TestApp::new()
        .routes(routes![handler])
        .idempotent()
        .build();

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

    let client = TestApp::new()
        .routes(routes![handler])
        .idempotent()
        .build();

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

    let client = TestApp::new()
        .routes(routes![handler])
        .idempotent()
        .build();

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

    let client = TestApp::new()
        .routes(routes![handler])
        .idempotent()
        .build();

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

    let client = TestApp::new()
        .routes(routes![handler])
        .idempotent()
        .build();

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

    let client = TestApp::new()
        .routes(routes![handler])
        .idempotent()
        .build();

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
    assert_eq!(config.ttl_secs, 86_400, "default TTL should be 86400 seconds");
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
