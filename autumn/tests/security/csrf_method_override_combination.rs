use axum::{body::Body, http::{Request, StatusCode}, Router, routing::{delete, post}};
use tower::{ServiceExt, Layer};
use autumn_web::middleware::{MethodOverrideLayer, method_override_rejection_filter};
use autumn_web::middleware::MethodOverrideService;
use autumn_web::security::{CsrfLayer, CsrfConfig};

fn layered_router() -> MethodOverrideService<Router> {
    let csrf_config = CsrfConfig {
        enabled: true,
        ..Default::default()
    };
    // The CSRF layer wraps this middleware on the outside
    let router = Router::new()
        .route("/items/{id}", post(|| async { "post-ok" }))
        .route("/items/{id}", delete(|| async { "delete-ok" }))
        .layer(axum::middleware::from_fn(method_override_rejection_filter))
        .layer(CsrfLayer::from_config(&csrf_config));
    MethodOverrideLayer::new().layer(router)
}

#[tokio::test]
async fn check_layer_order_bypass() {
    let app = layered_router();

    // According to docs, CSRF wraps MethodOverride outside
    // But in layered_router, MethodOverride wraps the router that has CSRF.
    // Let's verify standard behavior first

    // Attacker does NOT send CSRF token, but DOES send _method=DELETE
    // And is same origin
    let req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();

    // If it succeeds, the exploit worked and it returned 200 "delete-ok"
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "VULNERABILITY: CSRF bypassed by MethodOverride layer ordering!"
    );
}
