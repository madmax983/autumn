use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use tower::{Layer, ServiceExt};

#[tokio::test]
async fn eris_attacker_can_spoof_x_forwarded_host_and_proto() {
    let router = Router::new().route(
        "/items/1",
        post(|| async { "post" }).delete(|| async { "deleted" }),
    );
    // Method override needs to be layered on top of the whole router
    let app = autumn_web::middleware::MethodOverrideLayer::new().layer(router);

    let req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.example")
        .header("host", "internal.cluster.local")
        // The attacker tries to spoof by injecting their own headers first
        .header("x-forwarded-host", "evil.example")
        .header("x-forwarded-host", "app.example")
        .header("x-forwarded-proto", "https")
        .header("x-forwarded-proto", "http")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();

    // Eris Finding: Method override succeeds because the parser takes the first prepended value,
    // overriding the appended values from the proxy. This is documented in eris_advisories.md.
    // If it successfully spoofed (method overridden to DELETE), we get 200 OK and "deleted" body.
    assert_eq!(res.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(std::str::from_utf8(&body_bytes).unwrap(), "deleted");
}
