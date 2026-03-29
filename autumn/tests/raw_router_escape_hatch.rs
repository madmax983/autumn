//! Integration tests for the raw Axum router escape hatch (FR-041):
//! `AppBuilder::merge()` and `AppBuilder::nest()`.

use autumn_web::config::AutumnConfig;
use autumn_web::{AppState, get, routes};
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn test_state() -> AppState {
    AppState {
        #[cfg(feature = "db")]
        pool: None,
        profile: Some("test".into()),
        started_at: std::time::Instant::now(),
        health_detailed: false,
        metrics: autumn_web::middleware::MetricsCollector::new(),
        log_levels: autumn_web::actuator::LogLevels::new("info"),
        task_registry: autumn_web::actuator::TaskRegistry::new(),
        config_props: autumn_web::actuator::ConfigProperties::default(),
    }
}

// ── Merge tests ───────────────────────────────────────────────────

#[tokio::test]
async fn merged_route_is_accessible() {
    let raw = axum::Router::<AppState>::new()
        .route("/raw", axum::routing::get(|| async { "from raw router" }));

    let config = AutumnConfig::default();
    let router =
        autumn_web::app::build_router_merged(routes![], &config, test_state(), vec![raw], vec![]);

    let resp = router
        .oneshot(Request::builder().uri("/raw").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"from raw router");
}

#[tokio::test]
async fn merged_route_receives_app_state() {
    let raw = axum::Router::<AppState>::new().route(
        "/state-check",
        axum::routing::get(|State(state): State<AppState>| async move {
            format!("profile={}", state.profile())
        }),
    );

    let config = AutumnConfig::default();
    let router =
        autumn_web::app::build_router_merged(routes![], &config, test_state(), vec![raw], vec![]);

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/state-check")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"profile=test");
}

#[tokio::test]
async fn autumn_middleware_applies_to_merged_routes() {
    let raw =
        axum::Router::<AppState>::new().route("/raw-mid", axum::routing::get(|| async { "ok" }));

    let config = AutumnConfig::default();
    let router =
        autumn_web::app::build_router_merged(routes![], &config, test_state(), vec![raw], vec![]);

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/raw-mid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // RequestIdLayer adds x-request-id header
    assert!(
        resp.headers().get("x-request-id").is_some(),
        "Expected x-request-id header from Autumn middleware on merged route"
    );

    // SecurityHeadersLayer adds security headers
    assert!(
        resp.headers().get("x-content-type-options").is_some(),
        "Expected security headers from Autumn middleware on merged route"
    );
}

#[tokio::test]
async fn multiple_merged_routers_accumulate() {
    let raw1 =
        axum::Router::<AppState>::new().route("/raw1", axum::routing::get(|| async { "first" }));
    let raw2 =
        axum::Router::<AppState>::new().route("/raw2", axum::routing::get(|| async { "second" }));

    let config = AutumnConfig::default();
    let router = autumn_web::app::build_router_merged(
        routes![],
        &config,
        test_state(),
        vec![raw1, raw2],
        vec![],
    );

    let resp = router
        .clone()
        .oneshot(Request::builder().uri("/raw1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"first");

    let resp = router
        .oneshot(Request::builder().uri("/raw2").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"second");
}

// ── Nest tests ────────────────────────────────────────────────────

#[tokio::test]
async fn nested_route_works_under_prefix() {
    let raw = axum::Router::<AppState>::new()
        .route("/users", axum::routing::get(|| async { "v2 users" }));

    let config = AutumnConfig::default();
    let router = autumn_web::app::build_router_merged(
        routes![],
        &config,
        test_state(),
        vec![],
        vec![("/api/v2".to_owned(), raw)],
    );

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v2/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"v2 users");

    // The route should NOT be accessible without the prefix
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn nested_route_receives_app_state() {
    let raw = axum::Router::<AppState>::new().route(
        "/info",
        axum::routing::get(|State(state): State<AppState>| async move {
            format!("profile={}", state.profile())
        }),
    );

    let config = AutumnConfig::default();
    let router = autumn_web::app::build_router_merged(
        routes![],
        &config,
        test_state(),
        vec![],
        vec![("/nested".to_owned(), raw)],
    );

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/nested/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"profile=test");
}

#[tokio::test]
async fn nested_route_gets_autumn_middleware() {
    let raw =
        axum::Router::<AppState>::new().route("/check", axum::routing::get(|| async { "ok" }));

    let config = AutumnConfig::default();
    let router = autumn_web::app::build_router_merged(
        routes![],
        &config,
        test_state(),
        vec![],
        vec![("/nested".to_owned(), raw)],
    );

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/nested/check")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("x-request-id").is_some(),
        "Expected x-request-id header on nested route"
    );
}

// ── Precedence / coexistence tests ────────────────────────────────

#[get("/annotated")]
async fn annotated_handler() -> &'static str {
    "from annotated"
}

#[tokio::test]
async fn annotated_and_merged_routes_coexist() {
    // Annotated and merged routes on different paths work together.
    let raw = axum::Router::<AppState>::new()
        .route("/raw-only", axum::routing::get(|| async { "from raw" }));

    let config = AutumnConfig::default();
    let router = autumn_web::app::build_router_merged(
        routes![annotated_handler],
        &config,
        test_state(),
        vec![raw],
        vec![],
    );

    // Annotated route works
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/annotated")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"from annotated");

    // Merged route works
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/raw-only")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"from raw");
}

#[tokio::test]
async fn merged_route_adds_different_method_to_same_path() {
    // Annotated GET + merged POST on same path should work (Axum merges methods).
    let raw = axum::Router::<AppState>::new()
        .route("/annotated", axum::routing::post(|| async { "posted" }));

    let config = AutumnConfig::default();
    let router = autumn_web::app::build_router_merged(
        routes![annotated_handler],
        &config,
        test_state(),
        vec![raw],
        vec![],
    );

    // GET from annotated route
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/annotated")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"from annotated");

    // POST from merged route
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/annotated")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"posted");
}

// ── AppBuilder API compile tests ──────────────────────────────────

#[test]
fn app_builder_merge_compiles() {
    let raw =
        axum::Router::<AppState>::new().route("/ws", axum::routing::get(|| async { "websocket" }));

    let _builder = autumn_web::app()
        .routes(routes![annotated_handler])
        .merge(raw);
}

#[test]
fn app_builder_nest_compiles() {
    let raw =
        axum::Router::<AppState>::new().route("/users", axum::routing::get(|| async { "users" }));

    let _builder = autumn_web::app()
        .routes(routes![annotated_handler])
        .nest("/api/v2", raw);
}

#[test]
fn app_builder_multiple_merge_and_nest() {
    let raw1 = axum::Router::<AppState>::new().route("/a", axum::routing::get(|| async { "a" }));
    let raw2 = axum::Router::<AppState>::new().route("/b", axum::routing::get(|| async { "b" }));
    let nested = axum::Router::<AppState>::new().route("/c", axum::routing::get(|| async { "c" }));

    let _builder = autumn_web::app()
        .routes(routes![annotated_handler])
        .merge(raw1)
        .merge(raw2)
        .nest("/prefix", nested);
}

#[test]
fn app_state_accessible_for_external_router_construction() {
    // Users need to be able to write `Router::<AppState>::new()` externally.
    // This test verifies AppState is importable and usable as a type parameter.
    let _router = axum::Router::<autumn_web::AppState>::new()
        .route("/test", axum::routing::get(|| async { "test" }));
}
