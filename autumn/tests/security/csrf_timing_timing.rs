use autumn_web::security::{CsrfLayer, CsrfToken, csrf::CsrfConfig};
use axum::{routing::{get, post}, Router, body::Body, http::{Request, StatusCode}, response::IntoResponse};
use tower::ServiceExt;

#[tokio::test]
async fn eris_csrf_timing() {
    let mut config = CsrfConfig::default();
    config.enabled = true;

    let app = Router::new()
        .route("/submit", post(|| async { "created" }))
        .layer(CsrfLayer::from_config(&config));

    // Look at this length check: `let len_eq = a.len().ct_eq(&b.len());`
    // And then `(len_eq & bytes_eq).into()`
    // This is secure! Wait. Is there anything else?
}
