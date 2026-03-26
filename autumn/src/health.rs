//! Health check endpoint.
//!
//! Automatically mounted by [`AppBuilder::run`](crate::app::AppBuilder::run)
//! at the path configured in [`HealthConfig`](crate::config::HealthConfig)
//! (default: `/health`).
//!
//! # Response format
//!
//! **Without a database:**
//!
//! ```json
//! { "status": "ok", "version": "0.1.0" }
//! ```
//!
//! **With a database pool:**
//!
//! ```json
//! {
//!   "status": "ok",
//!   "version": "0.1.0",
//!   "pool": { "size": 10, "available": 8, "waiting": 0 }
//! }
//! ```
//!
//! Returns `200 OK` when healthy or `503 Service Unavailable` when the
//! database pool is exhausted (all connections in use **and** requests
//! are queuing).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::AppState;

/// Typed health response — avoids dynamic `serde_json::Value` allocation.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool: Option<PoolStatus>,
}

#[derive(Serialize)]
struct PoolStatus {
    size: u64,
    available: u64,
    waiting: u64,
}

/// Health check handler.
///
/// Returns pool status when a database is configured, or a simple
/// `"ok"` response when running without a database.
///
/// This handler is auto-mounted by the framework and does not need to be
/// registered manually.
///
/// # Response codes
///
/// - `200 OK` -- application is healthy.
/// - `503 Service Unavailable` -- database pool is exhausted (all
///   connections in use and requests are queuing).
#[allow(unused_variables)]
pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    #[cfg(feature = "db")]
    {
        if let Some(pool) = state.pool.as_ref() {
            let status = pool.status();
            let available = status.available as u64;
            let size = status.max_size as u64;
            let waiting = status.waiting as u64;

            // Degraded when all connections are in use AND requests are queuing
            let healthy = available > 0 || waiting == 0;

            let body = HealthResponse {
                status: if healthy { "ok" } else { "degraded" },
                version: env!("CARGO_PKG_VERSION"),
                pool: Some(PoolStatus {
                    size,
                    available,
                    waiting,
                }),
            };

            let status_code = if healthy {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };

            return (status_code, Json(body));
        }
    }

    let body = HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        pool: None,
    };
    (StatusCode::OK, Json(body))
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
            .with_state(AppState {
                #[cfg(feature = "db")]
                pool: None,
            });

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
            .with_state(AppState {
                #[cfg(feature = "db")]
                pool: None,
            });

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

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn health_with_pool_returns_pool_status() {
        // Build a pool from a dummy URL — it won't connect but the pool
        // status (max_size, available, waiting) is available immediately.
        let config = crate::config::DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            pool_size: 5,
            ..Default::default()
        };
        let pool = crate::db::create_pool(&config).unwrap().unwrap();

        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(AppState { pool: Some(pool) });

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
        assert_eq!(json["pool"]["size"], 5);
        assert!(json["pool"]["available"].is_number());
    }

    #[tokio::test]
    async fn health_response_includes_version() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(AppState {
                #[cfg(feature = "db")]
                pool: None,
            });

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
