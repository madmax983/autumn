use autumn_web::AppState;
use autumn_web::config::AutumnConfig;
use autumn_web::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn config_runtime_drift_actuator_prefix_is_mounted() {
    let mut config = AutumnConfig::default();
    config.actuator.prefix = "/ops".to_owned();

    let app = router::build_router(Vec::new(), &config, AppState::for_test());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ops/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/actuator/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
