use autumn_web::middleware::MethodOverrideLayer;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::delete,
};
use tower::ServiceExt;

#[tokio::test]
async fn method_override_origin_bypass_poc() {
    let app = Router::new()
        .route("/items/1", delete(|| async { "deleted" }))
        .layer(MethodOverrideLayer::new());

    let mut req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.com")
        .header("host", "legitimate.com") // Original host header
        .header("x-forwarded-proto", "https")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    // The attacker sets X-Forwarded-Host: evil.com
    req.headers_mut()
        .append("x-forwarded-host", "evil.com".parse().unwrap());

    // The proxy appends X-Forwarded-Host: legitimate.com
    req.headers_mut()
        .append("x-forwarded-host", "legitimate.com".parse().unwrap());

    let res = app.oneshot(req).await.unwrap();

    // With the patch, the method override should be rejected and fall through to the POST handler
    // Since there is no POST handler for /items/1, it returns 405 Method Not Allowed.
    assert_eq!(
        res.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "Origin bypass should be rejected"
    );
}
