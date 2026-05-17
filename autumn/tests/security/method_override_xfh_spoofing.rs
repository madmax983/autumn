use autumn_web::middleware::MethodOverrideLayer;
use axum::http::{Request, StatusCode};
use axum::{Router, body::Body, routing::delete};
use tower::ServiceExt;

#[tokio::test]
async fn method_override_xfh_spoofing() {
    let app = Router::new()
        .route("/items/1", delete(|| async { "Deleted" }))
        .layer(MethodOverrideLayer::new());

    // Attacker spoofs X-Forwarded-Host to match their malicious Origin
    let req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        // Attacker origin
        .header("origin", "https://attacker.com")
        // The real host
        .header("host", "victim.com")
        // Spoofed X-Forwarded-Host, followed by the legitimate proxy appended host
        .header("x-forwarded-host", "attacker.com, victim.com")
        .header("x-forwarded-proto", "https")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED, // Expected due to origin mismatch, previously returned 405 Method Not Allowed? Wait, if origin matches, it processes the override and returns OK or 403 (CSRF). If origin fails, it ignores the override, continuing as POST, resulting in 405 Method Not Allowed. Let's assert it doesn't process the override.
        "Vulnerable to X-Forwarded-Host spoofing!"
    );
}
