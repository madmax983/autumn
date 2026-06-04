use autumn_web::security::{CsrfConfig, CsrfLayer};
use axum::{body::Body, http::Request, routing::post, Router};
use tower::ServiceExt;

#[tokio::test]
async fn csrf_bypass_via_path_normalization() {
    let mut config = CsrfConfig::default();
    config.enabled = true;
    config.exempt_paths = vec!["/api/".to_string()];

    // A catch-all route that processes the logical path after CSRF middleware.
    // If an attacker sends `/api/../protected`, the raw path starts with `/api/`,
    // but the logical path is `/protected`.
    let app = Router::new()
        .route(
            "/{*path}",
            post(
                |axum::extract::Path(path): axum::extract::Path<String>| async move {
                    if path == "protected" {
                        "hacked".to_string()
                    } else {
                        "ok".to_string()
                    }
                },
            ),
        )
        .layer(CsrfLayer::from_config(&config));

    // Request with path traversal should be denied by CSRF middleware
    // because the normalized path `/protected` is not exempt.
    let req = Request::builder()
        .method("POST")
        .uri("/api/../protected")
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        axum::http::StatusCode::FORBIDDEN,
        "CSRF middleware should reject requests where the normalized path is not exempt"
    );

    // Another bypass attempt using percent-encoding
    let req = Request::builder()
        .method("POST")
        .uri("/api/%2e%2e/protected")
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        axum::http::StatusCode::FORBIDDEN,
        "CSRF middleware should reject requests where the percent-decoded normalized path is not exempt"
    );
}
