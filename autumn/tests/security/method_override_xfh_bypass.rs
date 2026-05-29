use autumn_web::middleware::MethodOverrideLayer;
use axum::{Router, body::Body, http::Request, routing::post};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get_body(res: axum::http::Response<axum::body::Body>) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn method_override_xfh_bypass() {
    let app = Router::new()
        .route(
            "/test",
            post(|| async { "POST" }).delete(|| async { "DELETE" }),
        )
        .layer(MethodOverrideLayer::new());

    // Attacker spoofs x-forwarded-host.
    let bypass_req = Request::builder()
        .method("POST")
        .uri("/test")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.example")
        .header("host", "internal.local")
        .header("x-forwarded-host", "evil.example")
        .header("x-forwarded-host", "app.example")
        .header("x-forwarded-proto", "https")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let res = app.clone().oneshot(bypass_req).await.unwrap();
    let body = get_body(res).await;

    assert_ne!(
        body, "DELETE",
        "Vulnerable to X-Forwarded-Host spoofing bypass"
    );
}

#[tokio::test]
async fn method_override_xfh_comma_separated() {
    let app = Router::new()
        .route(
            "/test",
            post(|| async { "POST" }).delete(|| async { "DELETE" }),
        )
        .layer(MethodOverrideLayer::new());

    // Single header, comma separated
    let bypass_req = Request::builder()
        .method("POST")
        .uri("/test")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "https://evil.example")
        .header("host", "internal.local")
        .header("x-forwarded-host", "evil.example, app.example")
        .header("x-forwarded-proto", "https")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let res = app.clone().oneshot(bypass_req).await.unwrap();
    let _status = res.status();
    let body = get_body(res).await;

    assert_ne!(
        body, "DELETE",
        "Vulnerable to X-Forwarded-Host spoofing bypass via commas"
    );
}
