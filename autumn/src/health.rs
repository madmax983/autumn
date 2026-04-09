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

use crate::state::AppState;

/// Typed health response — avoids dynamic `serde_json::Value` allocation.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uptime: Option<String>,
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
#[allow(
    unused_variables,
    clippy::if_not_else,
    clippy::needless_pass_by_value,
    clippy::useless_let_if_seq
)]
pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    let healthy;
    let pool_status;

    #[cfg(feature = "db")]
    {
        if let Some(pool) = state.pool.as_ref() {
            let status = pool.status();
            let available = status.available as u64;
            let size = status.max_size as u64;
            let waiting = status.waiting as u64;

            healthy = available > 0 || waiting == 0;
            pool_status = Some(PoolStatus {
                size,
                available,
                waiting,
            });
        } else {
            healthy = true;
            pool_status = None;
        }
    }

    #[cfg(not(feature = "db"))]
    {
        healthy = true;
        pool_status = None;
    }

    let detailed = state.health_detailed;
    let body = HealthResponse {
        status: if healthy { "ok" } else { "degraded" },
        version: if detailed {
            Some(env!("CARGO_PKG_VERSION"))
        } else {
            None
        },
        profile: if detailed {
            Some(state.profile().to_owned())
        } else {
            None
        },
        uptime: if detailed {
            Some(state.uptime_display())
        } else {
            None
        },
        pool: if detailed { pool_status } else { None },
    };

    let status_code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: Some("dev".into()),
            started_at: std::time::Instant::now(),
            health_detailed: true,
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
                pool: Some(pool),
                profile: Some("prod".into()),
                started_at: std::time::Instant::now(),
                health_detailed: true,
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
