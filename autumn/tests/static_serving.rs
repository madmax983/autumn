//! Tests for static file serving.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use tower_http::services::ServeDir;

#[tokio::test]
async fn serves_static_files_with_correct_mime_type() {
    let dir = tempfile::tempdir().unwrap();
    let css_dir = dir.path().join("css");
    std::fs::create_dir_all(&css_dir).unwrap();
    std::fs::write(css_dir.join("test.css"), "body { color: red; }").unwrap();

    let app = Router::new().nest_service("/static", ServeDir::new(dir.path()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/static/css/test.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/css"),
        "Expected text/css, got {content_type}"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"body { color: red; }");
}

#[tokio::test]
async fn missing_file_returns_404() {
    let dir = tempfile::tempdir().unwrap();
    let app = Router::new().nest_service("/static", ServeDir::new(dir.path()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/static/nonexistent.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn serves_subdirectories() {
    let dir = tempfile::tempdir().unwrap();
    let img_dir = dir.path().join("images").join("icons");
    std::fs::create_dir_all(&img_dir).unwrap();
    std::fs::write(img_dir.join("favicon.ico"), &[0u8; 10]).unwrap();

    let app = Router::new().nest_service("/static", ServeDir::new(dir.path()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/static/images/icons/favicon.ico")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
