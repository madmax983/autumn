use axum::{body::Body, http::Request, routing::{post, put}, Router};
use tower::ServiceExt;
use http_body_util::BodyExt;

#[tokio::test]
async fn eris_poc_method_override_forwarded_header_spoofing() {
    let app = Router::new()
        .route("/target", post(|| async { "POST" }))
        .route("/target", put(|| async { "PUT" }))
        .layer(autumn_web::middleware::MethodOverrideLayer::new());

    let req = Request::builder()
        .method("POST")
        .uri("/target")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.com")
        .header("x-forwarded-host", "evil.com, app.example")
        .header("x-forwarded-proto", "https, https")
        .body(Body::from("_method=PUT"))
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();

    assert_eq!(
        body_str,
        "POST",
        "VULNERABILITY: X-Forwarded-Host spoofing allowed the method override to execute! Expected 'POST' body, got '{body_str}'"
    );
}
