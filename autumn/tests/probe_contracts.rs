use autumn_web::AppState;
use autumn_web::config::AutumnConfig;
use autumn_web::route::Route;
use autumn_web::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use http::Method;
use tower::ServiceExt;

async fn assert_status(app: axum::Router, path: &str, expected: StatusCode) -> axum::Router {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), expected, "unexpected status for {path}");
    app
}

#[tokio::test]
async fn probe_contracts_default_probe_semantics() {
    let config = AutumnConfig::default();
    let state = AppState::for_test();
    state.set_startup_complete_for_test(false);

    let app = router::build_router(Vec::new(), &config, state);
    let app = assert_status(app, "/live", StatusCode::OK).await;
    let app = assert_status(app, "/ready", StatusCode::SERVICE_UNAVAILABLE).await;
    let app = assert_status(app, "/startup", StatusCode::SERVICE_UNAVAILABLE).await;
    let _app = assert_status(app, "/health", StatusCode::SERVICE_UNAVAILABLE).await;
}

#[tokio::test]
async fn probe_contracts_ready_turns_unavailable_during_shutdown() {
    let config = AutumnConfig::default();
    let state = AppState::for_test();

    let app = router::build_router(Vec::new(), &config, state.clone());
    let app = assert_status(app, "/ready", StatusCode::OK).await;
    let app = assert_status(app, "/health", StatusCode::OK).await;

    state.begin_shutdown_for_test();

    let app = assert_status(app, "/live", StatusCode::OK).await;
    let app = assert_status(app, "/ready", StatusCode::SERVICE_UNAVAILABLE).await;
    let _app = assert_status(app, "/health", StatusCode::SERVICE_UNAVAILABLE).await;
}

#[tokio::test]
async fn probe_contracts_probe_paths_are_configurable() {
    let mut config = AutumnConfig::default();
    config.health.path = "/healthz".to_owned();
    config.health.live_path = "/livez".to_owned();
    config.health.ready_path = "/readyz".to_owned();
    config.health.startup_path = "/startupz".to_owned();

    let state = AppState::for_test();
    state.set_startup_complete_for_test(false);

    let app = router::build_router(Vec::new(), &config, state);
    let app = assert_status(app, "/livez", StatusCode::OK).await;
    let app = assert_status(app, "/readyz", StatusCode::SERVICE_UNAVAILABLE).await;
    let app = assert_status(app, "/startupz", StatusCode::SERVICE_UNAVAILABLE).await;
    let app = assert_status(app, "/healthz", StatusCode::SERVICE_UNAVAILABLE).await;
    let _app = assert_status(app, "/live", StatusCode::SERVICE_UNAVAILABLE).await;
}

#[tokio::test]
async fn probe_contracts_user_routes_are_blocked_until_startup_completes() {
    let config = AutumnConfig::default();
    let state = AppState::for_test();
    state.set_startup_complete_for_test(false);

    let route = Route {
        method: Method::GET,
        path: "/",
        handler: get(|| async { "hello" }),
        name: "index",
    };

    let app = router::build_router(vec![route], &config, state);
    let app = assert_status(app, "/", StatusCode::SERVICE_UNAVAILABLE).await;
    let _app = assert_status(app, "/live", StatusCode::OK).await;
}

#[tokio::test]
async fn probe_contracts_static_router_without_dist_still_blocks_user_routes() {
    let config = AutumnConfig::default();
    let state = AppState::for_test();
    state.set_startup_complete_for_test(false);

    let route = Route {
        method: Method::GET,
        path: "/",
        handler: get(|| async { "hello" }),
        name: "index",
    };

    let app = router::build_router_with_static(vec![route], &config, state, None);
    let app = assert_status(app, "/", StatusCode::SERVICE_UNAVAILABLE).await;
    let _app = assert_status(app, "/live", StatusCode::OK).await;
}

#[tokio::test]
async fn probe_contracts_static_router_without_manifest_still_blocks_user_routes() {
    let config = AutumnConfig::default();
    let state = AppState::for_test();
    state.set_startup_complete_for_test(false);
    let dist = tempfile::tempdir().unwrap();

    let route = Route {
        method: Method::GET,
        path: "/",
        handler: get(|| async { "hello" }),
        name: "index",
    };

    let app = router::build_router_with_static(vec![route], &config, state, Some(dist.path()));
    let app = assert_status(app, "/", StatusCode::SERVICE_UNAVAILABLE).await;
    let _app = assert_status(app, "/live", StatusCode::OK).await;
}

#[tokio::test]
async fn probe_contracts_root_actuator_prefix_does_not_disable_startup_barrier() {
    let mut config = AutumnConfig::default();
    config.actuator.prefix = "/".to_owned();
    config.health.path = "/healthz".to_owned();
    config.health.live_path = "/livez".to_owned();
    config.health.ready_path = "/readyz".to_owned();
    config.health.startup_path = "/startupz".to_owned();

    let state = AppState::for_test();
    state.set_startup_complete_for_test(false);

    let route = Route {
        method: Method::GET,
        path: "/",
        handler: get(|| async { "hello" }),
        name: "index",
    };

    let app = router::build_router(vec![route], &config, state);
    let app = assert_status(app, "/", StatusCode::SERVICE_UNAVAILABLE).await;
    let _app = assert_status(app, "/metrics", StatusCode::OK).await;
}
