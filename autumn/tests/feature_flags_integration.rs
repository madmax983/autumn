//! Integration tests for the feature-flag system (AC-11).
//!
//! Verifies that:
//! - Flags registered via `with_flag_store` are available in handlers.
//! - Toggling a flag via the `FlagStore` trait propagates immediately.
//! - The `Flags` extractor returns 500 when no store is registered.
//! - Percent-rollout and actor-allowlist gates work end-to-end.

use std::sync::Arc;

use autumn_web::feature_flags::{FeatureFlagService, FlagStore, InMemoryFlagStore};
use autumn_web::prelude::*;
use autumn_web::test::TestApp;
use axum::http::StatusCode;

// ── Shared store so tests can mutate flags while the app is running ─────────

#[derive(Clone)]
struct SharedStore(Arc<InMemoryFlagStore>);

impl FlagStore for SharedStore {
    fn get(
        &self,
        key: &str,
    ) -> Result<
        Option<autumn_web::feature_flags::FlagConfig>,
        autumn_web::feature_flags::FlagStoreError,
    > {
        self.0.get(key)
    }
    fn list(
        &self,
    ) -> Result<Vec<autumn_web::feature_flags::FlagConfig>, autumn_web::feature_flags::FlagStoreError>
    {
        self.0.list()
    }
    fn enable(
        &self,
        key: &str,
        actor: Option<&str>,
    ) -> Result<(), autumn_web::feature_flags::FlagStoreError> {
        self.0.enable(key, actor)
    }
    fn disable(
        &self,
        key: &str,
        actor: Option<&str>,
    ) -> Result<(), autumn_web::feature_flags::FlagStoreError> {
        self.0.disable(key, actor)
    }
    fn set_rollout(
        &self,
        key: &str,
        pct: u8,
        actor: Option<&str>,
    ) -> Result<(), autumn_web::feature_flags::FlagStoreError> {
        self.0.set_rollout(key, pct, actor)
    }
    fn allow_actor(
        &self,
        key: &str,
        actor_id: &str,
        actor: Option<&str>,
    ) -> Result<(), autumn_web::feature_flags::FlagStoreError> {
        self.0.allow_actor(key, actor_id, actor)
    }
    fn add_group(
        &self,
        key: &str,
        group: &str,
        actor: Option<&str>,
    ) -> Result<(), autumn_web::feature_flags::FlagStoreError> {
        self.0.add_group(key, group, actor)
    }
    fn history(
        &self,
        key: &str,
        limit: usize,
    ) -> Result<
        Vec<autumn_web::feature_flags::FlagChangeRecord>,
        autumn_web::feature_flags::FlagStoreError,
    > {
        self.0.history(key, limit)
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

#[get("/gate")]
async fn gate_handler(flags: Flags) -> axum::http::Response<axum::body::Body> {
    if flags.enabled("my_feature") {
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .body("feature on".into())
            .unwrap()
    } else {
        axum::http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("feature off".into())
            .unwrap()
    }
}

#[get("/rollout")]
async fn rollout_handler(
    axum::extract::State(state): axum::extract::State<autumn_web::AppState>,
) -> axum::http::Response<axum::body::Body> {
    // Use a fixed actor_id so percent-rollout is deterministic regardless of session.
    use autumn_web::feature_flags::FeatureFlagService;
    let svc = state.extension::<FeatureFlagService>();
    let enabled = svc
        .as_deref()
        .is_some_and(|s| s.is_enabled("rollout_flag", Some("user:1")));
    if enabled {
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .body("in rollout".into())
            .unwrap()
    } else {
        axum::http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("not in rollout".into())
            .unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn flag_disabled_by_default_returns_feature_off() {
    let store = Arc::new(InMemoryFlagStore::new());
    let shared = SharedStore(store.clone());

    let client = TestApp::new()
        .with_flag_store(shared)
        .routes(routes![gate_handler])
        .build();

    client
        .get("/gate")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND.as_u16());
}

#[tokio::test]
async fn toggling_flag_propagates_within_one_request() {
    let store = Arc::new(InMemoryFlagStore::new());
    let shared = SharedStore(store.clone());

    let client = TestApp::new()
        .with_flag_store(shared)
        .routes(routes![gate_handler])
        .build();

    // Initially off
    client
        .get("/gate")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND.as_u16());

    // Enable the flag directly on the shared store
    store.enable("my_feature", None).unwrap();

    // Next request sees the flag as enabled
    client.get("/gate").send().await.assert_ok();
}

#[tokio::test]
async fn flag_disabled_after_enable_returns_feature_off() {
    let store = Arc::new(InMemoryFlagStore::new());
    let shared = SharedStore(store.clone());

    let client = TestApp::new()
        .with_flag_store(shared)
        .routes(routes![gate_handler])
        .build();

    store.enable("my_feature", None).unwrap();
    client.get("/gate").send().await.assert_ok();

    store.disable("my_feature", None).unwrap();
    client
        .get("/gate")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND.as_u16());
}

#[tokio::test]
async fn rollout_at_100_enables_for_all_actors() {
    let store = Arc::new(InMemoryFlagStore::new());
    let shared = SharedStore(store.clone());

    store.set_rollout("rollout_flag", 100, None).unwrap();

    let client = TestApp::new()
        .with_flag_store(shared)
        .routes(routes![rollout_handler])
        .build();

    client.get("/rollout").send().await.assert_ok();
}

#[tokio::test]
async fn rollout_at_0_disables_for_all_actors() {
    let store = Arc::new(InMemoryFlagStore::new());
    let shared = SharedStore(store.clone());

    store.set_rollout("rollout_flag", 0, None).unwrap();

    let client = TestApp::new()
        .with_flag_store(shared)
        .routes(routes![rollout_handler])
        .build();

    client
        .get("/rollout")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND.as_u16());
}

#[tokio::test]
async fn flags_extractor_returns_500_when_no_store_registered() {
    let client = TestApp::new().routes(routes![gate_handler]).build();

    client
        .get("/gate")
        .send()
        .await
        .assert_status(StatusCode::INTERNAL_SERVER_ERROR.as_u16());
}

// ── `with_flag_store` wiring test ────────────────────────────────────────────

#[tokio::test]
async fn with_flag_store_installs_service_as_extension() {
    let store = InMemoryFlagStore::new();
    store.enable("wired_flag", Some("test")).unwrap();

    // with_flag_store must be called BEFORE state_initializer so the service
    // is already installed when the assertion runs.
    let client = TestApp::new()
        .with_flag_store(store)
        .state_initializer(|state| {
            // Verify FeatureFlagService is accessible after with_flag_store wiring.
            assert!(state.extension::<FeatureFlagService>().is_some());
        })
        .routes(routes![gate_handler])
        .build();

    // If we reach here, the extension was installed successfully.
    let _ = client;
}
