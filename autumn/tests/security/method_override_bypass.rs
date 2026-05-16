use axum::{body::Body, http::Request, routing::delete};
use tower::ServiceExt;

#[tokio::test]
async fn x_forwarded_host_injection_is_rejected() {
    // Instead of TestApp::new().routes, we can use TestApp::from_app if it exists, or just build an axum Router.
    // The previous tests used Router directly for the middleware unit tests.
    // Let's just use `axum::Router` directly here.
    let app = axum::Router::new()
        .route("/items/1", delete(|| async { "deleted" }))
        .layer(autumn_web::middleware::MethodOverrideLayer::new());

    let mut req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://attacker.com")
        .header("x-forwarded-host", "attacker.com")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    req.headers_mut()
        .append("x-forwarded-host", "legitimate.com".parse().unwrap());

    let res = app.oneshot(req).await.unwrap();

    let status = res.status();
    assert!(
        status == axum::http::StatusCode::BAD_REQUEST
            || status == axum::http::StatusCode::METHOD_NOT_ALLOWED,
        "Method override should have been rejected! Status: {status}"
    );
}
