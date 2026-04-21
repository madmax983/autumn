use axum::{body::Body, http::Request, Router, routing::post};
use tower::ServiceExt;
use autumn_web::security::config::CsrfConfig;
use autumn_web::security::csrf::CsrfLayer;

#[tokio::test]
async fn csrf_cookie_has_secure_attribute() {
    let app = Router::new()
        .route("/submit", post(|| async { "created" }))
        .layer(CsrfLayer::from_config(&CsrfConfig {
            enabled: true,
            ..Default::default()
        }));

    let req = Request::builder()
        .uri("/submit")
        .method("POST")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();

    // Request fails CSRF, but generates a set-cookie header with a new token
    let set_cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap();

    // Cookie should contain "Secure"
    assert!(set_cookie.contains("Secure"), "Cookie should contain Secure flag after fix");
}
