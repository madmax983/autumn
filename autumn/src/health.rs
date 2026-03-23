//! Health check endpoint.
//!
//! Automatically mounted at the configured path (default: `/health`).
//! Returns JSON with application status and optional database pool metrics.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::AppState;

/// Health check handler.
///
/// Returns pool status when a database is configured, or a simple
/// "ok" response when running without a database.
///
/// - `200 OK` — application is healthy
/// - `503 Service Unavailable` — database pool is exhausted
pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    state.pool.as_ref().map_or_else(
        || {
            let body = serde_json::json!({
                "status": "ok",
                "version": env!("CARGO_PKG_VERSION"),
            });
            (StatusCode::OK, Json(body))
        },
        |pool| {
            let status = pool.status();
            let available = status.available as u64;
            let size = status.max_size as u64;
            let waiting = status.waiting as u64;

            // Degraded when all connections are in use AND requests are queuing
            let healthy = available > 0 || waiting == 0;

            let body = serde_json::json!({
                "status": if healthy { "ok" } else { "degraded" },
                "version": env!("CARGO_PKG_VERSION"),
                "pool": {
                    "size": size,
                    "available": available,
                    "waiting": waiting,
                }
            });

            let status_code = if healthy {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };

            (status_code, Json(body))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_no_database_returns_ok() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(AppState { pool: None });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "ok");
        assert!(json["version"].is_string());
        assert!(json.get("pool").is_none());
    }

    #[tokio::test]
    async fn health_no_database_returns_json_content_type() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(AppState { pool: None });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("application/json"),
            "Expected application/json, got {content_type}"
        );
    }

    #[tokio::test]
    async fn health_response_includes_version() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(AppState { pool: None });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }
}
