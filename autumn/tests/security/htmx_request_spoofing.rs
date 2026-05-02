use axum::{body::Body, http::Request, routing::get, Router};
use autumn_web::prelude::HxRequest;
use tower::ServiceExt;

async fn handler(hx: HxRequest) -> String {
    if hx.is_htmx {
        "htmx_response".to_string()
    } else {
        "full_page_response".to_string()
    }
}

#[tokio::test]
async fn test_hx_request_spoofing_changes_response() {
    let app = Router::new().route("/", get(handler));

    // A normal request gets the full page
    let normal_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let normal_body = axum::body::to_bytes(normal_response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&normal_body[..], b"full_page_response");

    // An attacker can trivially spoof the header to get the partial response
    let spoofed_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/")
                .header("hx-request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let spoofed_body = axum::body::to_bytes(spoofed_response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&spoofed_body[..], b"htmx_response");

    // As documented in [ERIS-NOTE], this is expected content negotiation and not a vulnerability
    // since it does not grant access to restricted data.
}
