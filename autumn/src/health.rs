//! Compatibility health endpoint.
//!
//! Automatically mounted by [`AppBuilder::run`](crate::app::AppBuilder::run) at
//! the path configured in [`HealthConfig`](crate::config::HealthConfig)
//! (default: `/health`). This endpoint is a compatibility alias for the
//! readiness probe during the `v0.x` transition to explicit `/live`,
//! `/ready`, and `/startup` probe contracts.
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
//! Returns the same response as the readiness probe.

use crate::state::AppState;
use axum::extract::State;
use axum::response::IntoResponse;

/// Health check handler.
///
/// Returns the readiness response at the compatibility `/health` path.
///
/// This handler is auto-mounted by the framework and does not need to be
/// registered manually.
///
/// # Response codes
///
/// - `200 OK` -- application is healthy.
/// - `503 Service Unavailable` -- database pool is exhausted (all
///   connections in use and requests are queuing).
pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    crate::probe::readiness_response(&state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            extensions: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            #[cfg(feature = "db")]
            pool: None,
            profile: Some("dev".into()),
            started_at: std::time::Instant::now(),
            health_detailed: true,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new("info"),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn health_no_database_returns_ok() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(test_state());

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
        assert_eq!(json["profile"], "dev");
        assert!(json["uptime"].is_string());
        assert!(json.get("pool").is_none());
    }

    #[tokio::test]
    async fn health_no_database_returns_json_content_type() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(test_state());

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
        let config = crate::config::DatabaseConfig {
            url: Some("postgres://localhost/test".into()),
            pool_size: 5,
            ..Default::default()
        };
        let pool = crate::db::create_pool(&config).unwrap().unwrap();

        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(AppState {
                extensions: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
                pool: Some(pool),
                profile: Some("prod".into()),
                started_at: std::time::Instant::now(),
                health_detailed: true,
                probes: crate::probe::ProbeState::ready_for_test(),
                metrics: crate::middleware::MetricsCollector::new(),
                log_levels: crate::actuator::LogLevels::new("info"),
                task_registry: crate::actuator::TaskRegistry::new(),
                config_props: crate::actuator::ConfigProperties::default(),
                #[cfg(feature = "ws")]
                channels: crate::channels::Channels::new(32),
                #[cfg(feature = "ws")]
                shutdown: tokio_util::sync::CancellationToken::new(),
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
        assert_eq!(json["pool"]["size"], 5);
        assert!(json["pool"]["available"].is_number());
    }

    #[tokio::test]
    async fn health_response_includes_version() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(test_state());

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

    #[tokio::test]
    async fn health_detailed_false_omits_details() {
        let mut state = test_state();
        state.health_detailed = false;

        let app = axum::Router::new()
            .route("/health", axum::routing::get(handler))
            .with_state(state);

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
        assert!(json.get("version").is_none());
        assert!(json.get("profile").is_none());
        assert!(json.get("uptime").is_none());
        assert!(json.get("pool").is_none());
    }
}
