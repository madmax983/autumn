use autumn_macros::get;
use autumn_web::test::TestApp;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[get("/")]
pub async fn index() -> &'static str {
    "hello"
}

#[tokio::test]
async fn test_fallback_middleware_bypass_csrf() {
    let mut config = autumn_web::config::AutumnConfig::default();
    config.security.csrf.enabled = true; // ensure CSRF is enabled

    let app = TestApp::new()
        .config(config)
        .routes(autumn_web::routes![index])
        .build();
    let router = app.into_router();

    let req = Request::builder()
        .method("POST")
        .uri("/nonexistent")
        .body(Body::empty())
        .unwrap();

    let res = router.clone().oneshot(req).await.unwrap();

    // CSRF should apply to the 404 fallback route as well and return 403.
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_valid_route_csrf() {
    let mut config = autumn_web::config::AutumnConfig::default();
    config.security.csrf.enabled = true; // ensure CSRF is enabled

    let app = TestApp::new()
        .config(config)
        .routes(autumn_web::routes![index])
        .build();
    let router = app.into_router();

    let req = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::empty())
        .unwrap();

    let res = router.clone().oneshot(req).await.unwrap();

    // valid routes are protected and return 403
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
