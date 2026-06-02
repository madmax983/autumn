use axum::{body::Body, http::{Request, StatusCode}, Router, routing::{delete, post}};
use tower::{ServiceExt, Layer};
use autumn_web::middleware::{MethodOverrideLayer, method_override_rejection_filter};
use autumn_web::middleware::MethodOverrideService;

fn layered_router() -> MethodOverrideService<Router> {
    let router = Router::new()
        .route("/items/{id}", post(|| async { "post-ok" }))
        .route("/items/{id}", delete(|| async { "delete-ok" }))
        .layer(axum::middleware::from_fn(method_override_rejection_filter));
    MethodOverrideLayer::new().layer(router)
}

#[tokio::test]
async fn attacker_spoofed_x_forwarded_proto_prepend() {
    let app = layered_router();
    let mut req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "http://app.example")
        .header("host", "app.example")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    // Reverse proxy adds X-Forwarded-Proto: https
    // But attacker previously sent X-Forwarded-Proto: http
    // So the final header is http, https (or separate headers)
    req.headers_mut().append("x-forwarded-proto", "http".parse().unwrap());
    req.headers_mut().append("x-forwarded-proto", "https".parse().unwrap());

    let response = app.oneshot(req).await.unwrap();

    // Attacker is http, proxy is https. If it accepts it, the attacker successfully tricked the proxy
    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "VULNERABILITY: Spoofed X-Forwarded-Proto was accepted!"
    );
}

#[tokio::test]
async fn attacker_spoofed_x_forwarded_proto_comma() {
    let app = layered_router();
    let req = Request::builder()
        .method("POST")
        .uri("/items/1")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "http://app.example")
        .header("host", "app.example")
        // Attacker sends http, proxy appends https
        .header("x-forwarded-proto", "http, https")
        .body(Body::from("_method=DELETE"))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();

    // Attacker is http, proxy is https. If it accepts it, the attacker successfully tricked the proxy
    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "VULNERABILITY: Spoofed comma-separated X-Forwarded-Proto was accepted!"
    );
}
