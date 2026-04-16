use tower::ServiceExt;

#[tokio::test]
async fn eris_unauthenticated_harvest_api() {
    use autumn_web::AppState;
    use autumn_web::reexports::axum::body::Body;
    use autumn_web::reexports::http::{Request, StatusCode};
    use autumn_web_harvest::api::{HarvestApiState, harvest_api_router};

    let api_state = HarvestApiState::new();
    let app = harvest_api_router(api_state).with_state(AppState::for_test());

    let req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn eris_unauthenticated_harvest_api_start_workflow() {
    use autumn_web::AppState;
    use autumn_web::reexports::axum::body::Body;
    use autumn_web::reexports::http::{Request, StatusCode};
    use autumn_web_harvest::api::{HarvestApiState, harvest_api_router};

    let api_state = HarvestApiState::new();
    let app = harvest_api_router(api_state).with_state(AppState::for_test());

    let req = Request::builder()
        .method("POST")
        .uri("/workflows/some-workflow/start")
        .header("Content-Type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();

    assert_ne!(res.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn eris_authenticated_harvest_api_start_workflow() {
    use autumn_web::auth::RequireAuth;
    use autumn_web::reexports::axum::body::Body;
    use autumn_web::reexports::http::{Request, StatusCode};

    let api_state = autumn_web_harvest::api::HarvestApiState::new();
    let app = autumn_web_harvest::api::harvest_api_router(api_state)
        .route_layer(RequireAuth::new("admin_id"))
        .with_state(autumn_web::AppState::for_test());

    let req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();

    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
