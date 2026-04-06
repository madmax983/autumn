//! Actuator endpoints for operational observability.
//!
//! Provides health, info, env, metrics, configprops, loggers, and tasks
//! endpoints at `/actuator/*`.
//!
//! Sensitive endpoints are gated by profile-aware defaults:
//! - **dev**: all endpoints enabled
//! - **prod**: only health, info, and metrics

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

// ── Health ──────────────────────────────────────────────────────

/// Enhanced health response for the actuator health endpoint.
#[derive(Serialize)]
struct ActuatorHealth {
    status: &'static str,
    version: &'static str,
    profile: String,
    uptime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checks: Option<HealthChecks>,
}

#[derive(Serialize)]
struct HealthChecks {
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<DatabaseCheck>,
}

#[derive(Serialize)]
struct DatabaseCheck {
    status: &'static str,
    pool_size: u64,
    active_connections: u64,
    idle_connections: u64,
}

/// `GET /actuator/health`
#[allow(unused_variables, clippy::useless_let_if_seq)]
pub async fn health(State(state): State<crate::state::AppState>) -> impl IntoResponse {
    let db_check;
    let overall_healthy;

    #[cfg(feature = "db")]
    {
        if let Some(pool) = state.pool.as_ref() {
            let status = pool.status();
            let available = status.available as u64;
            let size = status.max_size as u64;
            let waiting = status.waiting as u64;
            let idle = available;
            let active = size.saturating_sub(available);

            overall_healthy = available > 0 || waiting == 0;
            db_check = Some(DatabaseCheck {
                status: if overall_healthy { "ok" } else { "down" },
                pool_size: size,
                active_connections: active,
                idle_connections: idle,
            });
        } else {
            overall_healthy = true;
            db_check = None;
        }
    }

    #[cfg(not(feature = "db"))]
    {
        overall_healthy = true;
        db_check = None;
    }

    let checks = db_check.map(|db| HealthChecks { database: Some(db) });

    let body = ActuatorHealth {
        status: if overall_healthy { "ok" } else { "degraded" },
        version: env!("CARGO_PKG_VERSION"),
        profile: state.profile().to_owned(),
        uptime: state.uptime_display(),
        checks,
    };

    let code = if overall_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

// ── Info ────────────────────────────────────────────────────────

/// Application info response.
#[derive(Serialize)]
pub(crate) struct ActuatorInfo {
    app: AppInfo,
    autumn: FrameworkInfo,
    runtime: RuntimeInfo,
}

#[derive(Serialize)]
struct AppInfo {
    name: String,
    version: String,
}

#[derive(Serialize)]
struct FrameworkInfo {
    version: &'static str,
    profile: String,
}

#[derive(Serialize)]
struct RuntimeInfo {
    uptime: String,
}

/// `GET /actuator/info`
pub(crate) async fn info(State(state): State<crate::state::AppState>) -> Json<ActuatorInfo> {
    Json(ActuatorInfo {
        app: AppInfo {
            name: std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "unknown".into()),
            version: std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into()),
        },
        autumn: FrameworkInfo {
            version: env!("CARGO_PKG_VERSION"),
            profile: state.profile().to_owned(),
        },
        runtime: RuntimeInfo {
            uptime: state.uptime_display(),
        },
    })
}

// ── Env (sensitive) ─────────────────────────────────────────────

/// Config environment response with redacted secrets.
#[derive(Serialize)]
pub(crate) struct ActuatorEnv {
    active_profile: String,
    properties: std::collections::HashMap<String, serde_json::Value>,
}

/// `GET /actuator/env` — only available when actuator sensitive mode is enabled.
pub(crate) async fn env_endpoint(State(state): State<crate::state::AppState>) -> Json<ActuatorEnv> {
    let props = vec![
        ("server.host", "127.0.0.1"),
        ("server.port", "3000"),
        ("log.level", "info"),
        ("log.format", "Auto"),
        ("health.path", "/health"),
        ("database.url", "postgres://..."),
    ];

    let redacted: std::collections::HashMap<String, serde_json::Value> = props
        .into_iter()
        .map(|(k, v)| {
            let val = if crate::diagnostics::should_redact(k) {
                serde_json::Value::String("****".into())
            } else {
                serde_json::Value::String(v.into())
            };
            (k.to_string(), val)
        })
        .collect();

    Json(ActuatorEnv {
        active_profile: state.profile().to_owned(),
        properties: redacted,
    })
}

// ── Metrics ────────────────────────────────────────────────────

/// `GET /actuator/metrics` -- request metrics, latency, status codes, DB pool stats.
#[allow(unused_variables, unused_mut)]
pub(crate) async fn metrics_endpoint(
    State(state): State<crate::state::AppState>,
) -> Json<serde_json::Value> {
    let snapshot = state.metrics().snapshot();
    let mut result = serde_json::to_value(&snapshot).unwrap_or_default();

    // Include DB pool stats if available
    #[cfg(feature = "db")]
    if let Some(pool) = state.pool.as_ref() {
        let status = pool.status();
        let db_stats = serde_json::json!({
            "pool_size": status.max_size,
            "active_connections": (status.max_size as u64).saturating_sub(status.available as u64),
            "idle_connections": status.available,
        });
        if let serde_json::Value::Object(ref mut map) = result {
            map.insert("database".to_string(), db_stats);
        }
    }

    Json(result)
}

// ── Config Properties (sensitive) ──────────────────────────────

/// `GET /actuator/configprops` -- all config properties with source tracking.
pub(crate) async fn configprops_endpoint(
    State(state): State<crate::state::AppState>,
) -> Json<serde_json::Value> {
    let props = state.config_props().snapshot();

    Json(serde_json::json!({
        "active_profile": state.profile(),
        "properties": props,
    }))
}

// ── Loggers (sensitive) ────────────────────────────────────────

/// Available log levels for the loggers endpoint.
const AVAILABLE_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];

/// Response for `GET /actuator/loggers`.
#[derive(Serialize)]
pub(crate) struct LoggersResponse {
    current_level: String,
    available_levels: Vec<&'static str>,
    loggers: HashMap<String, String>,
}

/// `GET /actuator/loggers` -- view current log levels.
pub(crate) async fn loggers_get(
    State(state): State<crate::state::AppState>,
) -> Json<LoggersResponse> {
    Json(LoggersResponse {
        current_level: state.log_levels().current_level(),
        available_levels: AVAILABLE_LEVELS.to_vec(),
        loggers: state.log_levels().logger_overrides(),
    })
}

/// Request body for `PUT /actuator/loggers/{name}`.
#[derive(Deserialize)]
pub(crate) struct SetLoggerRequest {
    level: String,
}

/// `PUT /actuator/loggers/{name}` -- change a logger's level at runtime.
pub(crate) async fn loggers_put(
    State(state): State<crate::state::AppState>,
    Path(name): Path<String>,
    Json(body): Json<SetLoggerRequest>,
) -> impl IntoResponse {
    let level = body.level.to_lowercase();

    // Validate the level
    if !AVAILABLE_LEVELS.contains(&level.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": format!(
                    "Invalid level '{}'. Available levels: {}",
                    level,
                    AVAILABLE_LEVELS.join(", ")
                ),
            })),
        );
    }

    let previous = state.log_levels().set_logger_level(&name, &level);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok",
            "message": format!("Logger '{}' set to '{}'", name, level),
            "previous": previous,
        })),
    )
}

// ── Tasks (sensitive) ──────────────────────────────────────────

/// `GET /actuator/tasks` -- scheduled task status.
pub(crate) async fn tasks_endpoint(
    State(state): State<crate::state::AppState>,
) -> Json<serde_json::Value> {
    let tasks = state.task_registry().snapshot();

    Json(serde_json::json!({
        "scheduled_tasks": tasks,
    }))
}

// ── Tasks Stream (WebSocket) ───────────────────────────────────

/// `GET /actuator/tasks/stream` -- stream scheduled task events.
#[cfg(feature = "ws")]
pub(crate) async fn tasks_stream_endpoint(
    State(state): State<crate::state::AppState>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        let mut rx = state.channels().subscribe("sys:tasks");
        let shutdown = state.shutdown_token();

        loop {
            tokio::select! {
                res = rx.recv() => {
                    match res {
                        Ok(msg) => {
                            let ws_msg = axum::extract::ws::Message::Text(msg.into_string().into());
                            if socket.send(ws_msg).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                () = shutdown.cancelled() => {
                    let _ = socket.send(axum::extract::ws::Message::Close(None)).await;
                    break;
                }
                else => break,
            }
        }
    })
}

// ── Router builder ──────────────────────────────────────────────

/// Build the actuator router with profile-aware endpoint exposure.
///
/// In dev mode (or when `actuator.sensitive = true`), all endpoints are
/// exposed. In prod mode, only health, info, and metrics are available.
pub fn actuator_router(sensitive: bool) -> axum::Router<crate::state::AppState> {
    let mut router = axum::Router::new()
        .route("/actuator/health", axum::routing::get(health))
        .route("/actuator/info", axum::routing::get(info))
        .route("/actuator/metrics", axum::routing::get(metrics_endpoint));

    if sensitive {
        router = router
            .route("/actuator/env", axum::routing::get(env_endpoint))
            .route(
                "/actuator/configprops",
                axum::routing::get(configprops_endpoint),
            )
            .route("/actuator/loggers", axum::routing::get(loggers_get))
            .route("/actuator/loggers/{name}", axum::routing::put(loggers_put))
            .route("/actuator/tasks", axum::routing::get(tasks_endpoint));

        #[cfg(feature = "ws")]
        {
            router = router.route(
                "/actuator/tasks/stream",
                axum::routing::get(tasks_stream_endpoint),
            );
        }
    }

    router
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state() -> crate::state::AppState {
        crate::state::AppState {
            #[cfg(feature = "db")]
            pool: None,
            profile: Some("dev".into()),
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: LogLevels::new("info"),
            task_registry: TaskRegistry::new(),
            config_props: ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn actuator_health_returns_ok() {
        let app = actuator_router(true).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["profile"], "dev");
        assert!(json["uptime"].is_string());
    }

    #[tokio::test]
    async fn actuator_info_returns_metadata() {
        let app = actuator_router(true).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["autumn"]["version"].is_string());
        assert_eq!(json["autumn"]["profile"], "dev");
    }

    #[tokio::test]
    async fn actuator_env_available_in_sensitive_mode() {
        let app = actuator_router(true).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/env")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn actuator_env_hidden_in_nonsensitive_mode() {
        let app = actuator_router(false).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/env")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn redaction_patterns() {
        assert!(crate::diagnostics::should_redact("database.url"));
        assert!(crate::diagnostics::should_redact("api_token"));
        assert!(crate::diagnostics::should_redact("secret_key"));
        assert!(!crate::diagnostics::should_redact("server.port"));
        assert!(!crate::diagnostics::should_redact("log.level"));
    }

    // ── Metrics endpoint tests ─────────────────────────────────

    #[tokio::test]
    async fn actuator_metrics_returns_http_stats() {
        let state = test_state();
        state.metrics().record("GET", "/test", 200, 10);
        state.metrics().record("POST", "/test", 500, 50);

        let app = actuator_router(true).with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["http"]["requests_total"], 2);
        assert_eq!(json["http"]["by_status"]["2xx"], 1);
        assert_eq!(json["http"]["by_status"]["5xx"], 1);
    }

    #[tokio::test]
    async fn actuator_metrics_available_in_nonsensitive_mode() {
        let app = actuator_router(false).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Config properties endpoint tests ───────────────────────

    #[tokio::test]
    async fn actuator_configprops_returns_properties() {
        let app = actuator_router(true).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/configprops")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["active_profile"], "dev");
        assert!(json["properties"].is_object());
    }

    #[tokio::test]
    async fn actuator_configprops_hidden_in_nonsensitive_mode() {
        let app = actuator_router(false).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/configprops")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn configprops_redacts_sensitive_values() {
        let mut props = HashMap::new();
        ConfigProperties::track_property(
            &mut props,
            "database.url",
            "postgres://user:pass@host/db",
            "",
            "dev",
        );
        assert_eq!(props["database.url"].value, "****");
    }

    #[test]
    fn configprops_tracks_default_source() {
        let mut props = HashMap::new();
        ConfigProperties::track_property(&mut props, "server.port", "3000", "3000", "dev");
        assert_eq!(props["server.port"].source, "default");
        assert_eq!(props["server.port"].value, "3000");
    }

    #[test]
    fn configprops_tracks_profile_source() {
        let mut props = HashMap::new();
        ConfigProperties::track_property(&mut props, "log.level", "debug", "info", "dev");
        assert_eq!(props["log.level"].source, "profile_default:dev");
    }

    // ── Loggers endpoint tests ─────────────────────────────────

    #[tokio::test]
    async fn actuator_loggers_get_returns_levels() {
        let app = actuator_router(true).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/loggers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["current_level"], "info");
        assert!(json["available_levels"].is_array());
    }

    #[tokio::test]
    async fn actuator_loggers_put_changes_level() {
        let state = test_state();
        let app = actuator_router(true).with_state(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/actuator/loggers/autumn_web")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"level": "debug"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["message"], "Logger 'autumn_web' set to 'debug'");

        let overrides = state.log_levels().logger_overrides();
        assert_eq!(
            overrides.get("autumn_web").map(String::as_str),
            Some("debug")
        );
    }

    #[tokio::test]
    async fn actuator_loggers_put_rejects_invalid_level() {
        let app = actuator_router(true).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/actuator/loggers/autumn_web")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"level": "banana"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "error");
    }

    #[tokio::test]
    async fn actuator_loggers_hidden_in_nonsensitive_mode() {
        let app = actuator_router(false).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/loggers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn log_levels_set_and_get() {
        let levels = LogLevels::new("info");
        assert_eq!(levels.current_level(), "info");

        let _ = levels.set_logger_level("my_crate", "debug");
        let overrides = levels.logger_overrides();
        assert_eq!(overrides.get("my_crate").map(String::as_str), Some("debug"));
    }

    #[test]
    fn log_levels_root_updates_current() {
        let levels = LogLevels::new("info");
        let prev = levels.set_logger_level("root", "trace");
        assert_eq!(prev, Some("info".to_string()));
        assert_eq!(levels.current_level(), "trace");
    }

    // ── Tasks endpoint tests ───────────────────────────────────

    #[tokio::test]
    async fn actuator_tasks_returns_registered_tasks() {
        let state = test_state();
        state.task_registry().register("cleanup", "every 5m");
        state.task_registry().record_start("cleanup");
        state.task_registry().record_success("cleanup", 150);

        let app = actuator_router(true).with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let task = &json["scheduled_tasks"]["cleanup"];
        assert_eq!(task["schedule"], "every 5m");
        assert_eq!(task["status"], "idle");
        assert_eq!(task["total_runs"], 1);
        assert_eq!(task["total_failures"], 0);
        assert_eq!(task["last_result"], "ok");
        assert_eq!(task["last_duration_ms"], 150);
    }

    #[tokio::test]
    async fn actuator_tasks_hidden_in_nonsensitive_mode() {
        let app = actuator_router(false).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn task_registry_records_failure() {
        let registry = TaskRegistry::new();
        registry.register("my_task", "cron 0 * * * *");
        registry.record_start("my_task");
        registry.record_failure("my_task", 200, "connection refused");

        let snapshot = registry.snapshot();
        let task = &snapshot["my_task"];
        assert_eq!(task.status, "idle");
        assert_eq!(task.total_runs, 1);
        assert_eq!(task.total_failures, 1);
        assert_eq!(task.last_result.as_deref(), Some("failed"));
        assert_eq!(task.last_error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn task_registry_empty_snapshot() {
        let registry = TaskRegistry::new();
        assert!(registry.snapshot().is_empty());
    }
    #[test]
    fn log_levels_rejects_new_key_at_capacity() {
        let levels = LogLevels::new("info");
        // Fill to capacity
        for i in 0..1000 {
            let _ = levels.set_logger_level(&format!("logger_{i}"), "debug");
        }

        // Try to add a new key, should be rejected
        let result = levels.set_logger_level("logger_1000", "warn");
        assert_eq!(result, None);
        assert_eq!(levels.logger_overrides().len(), 1000);
        assert_eq!(levels.logger_overrides().get("logger_1000"), None);
    }

    #[test]
    fn log_levels_accepts_existing_key_at_capacity() {
        let levels = LogLevels::new("info");
        // Fill to capacity
        for i in 0..1000 {
            let _ = levels.set_logger_level(&format!("logger_{i}"), "debug");
        }

        // Try to update an existing key, should succeed
        let prev = levels.set_logger_level("logger_999", "warn");
        assert_eq!(prev.as_deref(), Some("debug"));
        assert_eq!(levels.logger_overrides().len(), 1000);
        assert_eq!(
            levels
                .logger_overrides()
                .get("logger_999")
                .map(String::as_str),
            Some("warn")
        );
    }

    #[test]
    fn task_registry_records_multiple_successes_and_failures() {
        let registry = TaskRegistry::new();
        registry.register("my_task", "cron * * * * *");

        // 1st success
        registry.record_start("my_task");
        registry.record_success("my_task", 100);

        // 2nd success
        registry.record_start("my_task");
        registry.record_success("my_task", 110);

        let snapshot = registry.snapshot();
        let task = &snapshot["my_task"];
        assert_eq!(task.total_runs, 2);
        assert_eq!(task.total_failures, 0);

        // 1st failure
        registry.record_start("my_task");
        registry.record_failure("my_task", 50, "failed");

        let snapshot2 = registry.snapshot();
        let task2 = &snapshot2["my_task"];
        assert_eq!(task2.total_runs, 3);
        assert_eq!(task2.total_failures, 1);
    }

    #[test]
    fn configprops_tracks_custom_profile() {
        let mut props = HashMap::new();
        ConfigProperties::track_property(
            &mut props,
            "log.level",
            "debug",
            "info",
            "custom_profile",
        );
        assert_eq!(props["log.level"].source, "autumn.toml");
    }

    #[test]
    fn configprops_tracks_dev_prod_profiles() {
        let mut props = HashMap::new();
        ConfigProperties::track_property(&mut props, "log.level", "debug", "info", "dev");
        assert_eq!(props["log.level"].source, "profile_default:dev");

        ConfigProperties::track_property(&mut props, "log.format", "json", "text", "prod");
        assert_eq!(props["log.format"].source, "profile_default:prod");
    }

    #[test]
    fn configprops_returns_default_when_values_match() {
        let mut props = HashMap::new();
        ConfigProperties::track_property(&mut props, "log.level", "info", "info", "dev");
        assert_eq!(props["log.level"].source, "default");
    }
}

#[cfg(test)]
mod havoc_proptest {
    use crate::diagnostics::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1))]
        #[test]
        fn log_levels_memory_exhaustion(names in proptest::collection::vec(".*", 5000)) {
            let levels = LogLevels::new("info");
            for name in names {
                let _ = levels.set_logger_level(&name, "debug");
            }
            assert!(levels.logger_overrides().len() <= 1000, "Memory leak: unbounded loggers inserted");
        }
    }
}
