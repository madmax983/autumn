use axum::{body::Body, http::{Request, HeaderValue}, routing::get, Router};
use autumn_web::prelude::HxResponseExt;
use tower::ServiceExt;

async fn handler() -> impl axum::response::IntoResponse {
    // Attempt CRLF injection
    "ok".hx_trigger("event\r\nInjected-Header: injected")
}

#[tokio::test]
async fn test_htmx_crlf_blocked_by_headervalue() {
    let app = Router::new().route("/", get(handler));

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let headers = response.headers();

    // The injected header should not be present
    assert!(headers.get("Injected-Header").is_none());

    // The hx-trigger header itself should not be present because `HeaderValue::from_str` failed and the error was swallowed
    assert!(headers.get("hx-trigger").is_none());

    // Verify HeaderValue blocks this at a fundamental level
    let res = HeaderValue::from_str("event\r\nInjected-Header: injected");
    assert!(res.is_err());
}
