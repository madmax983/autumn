//! Actuator endpoints for operational observability.
//!
//! Provides health, info, env, metrics, configprops, loggers, and tasks
//! endpoints at `/actuator/*`.
//!
//! Sensitive endpoints are gated by profile-aware defaults:
//! - **dev**: all endpoints enabled
//! - **prod**: only health, info, and metrics

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::AppState;

// ── Shared types for AppState ──────────────────────────────────

/// Runtime log level management for the `/actuator/loggers` endpoint.
///
/// Stores the current effective log level and per-logger overrides.
/// Changes are ephemeral -- they reset on restart.
#[derive(Clone)]
pub struct LogLevels {
    inner: Arc<RwLock<LogLevelsInner>>,
}

struct LogLevelsInner {
    /// The current global log level.
    current_level: String,
    /// Per-logger level overrides applied at runtime.
    logger_overrides: HashMap<String, String>,
}

impl LogLevels {
    /// Create a new `LogLevels` with the given initial level.
    #[must_use]
    pub fn new(initial_level: &str) -> Self {
        Self {
            inner: Arc::new(RwLock::new(LogLevelsInner {
                current_level: initial_level.to_string(),
                logger_overrides: HashMap::new(),
            })),
        }
    }

    /// Get the current global log level.
    #[must_use]
    pub fn current_level(&self) -> String {
        self.inner
            .read()
            .map_or_else(|_| "info".to_string(), |guard| guard.current_level.clone())
    }

    /// Get all per-logger overrides.
    #[must_use]
    pub fn logger_overrides(&self) -> HashMap<String, String> {
        self.inner
            .read()
            .map(|guard| guard.logger_overrides.clone())
            .unwrap_or_default()
    }

    /// Set the level for a specific logger. Returns the previous level if any.
    #[must_use]
    pub fn set_logger_level(&self, name: &str, level: &str) -> Option<String> {
        if let Ok(mut guard) = self.inner.write() {
            let previous = guard.logger_overrides.get(name).cloned();
            guard
                .logger_overrides
                .insert(name.to_string(), level.to_string());
            // If setting the root level, update current_level too
            if name == "root" || name.is_empty() {
                let prev = Some(guard.current_level.clone());
                guard.current_level = level.to_string();
                return prev;
            }
            previous
        } else {
            None
        }
    }
}

impl std::fmt::Debug for LogLevels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogLevels")
            .field("current_level", &self.current_level())
            .finish()
    }
}

/// Scheduled task status information.
#[derive(Debug, Clone, Serialize)]
pub struct TaskStatus {
    /// The schedule description (e.g., "every 5m" or "cron 0 0 * * *").
    pub schedule: String,
    /// Current task state.
    pub status: String,
    /// Last time the task ran (ISO 8601), if ever.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    /// Duration of last run in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
    /// Result of last run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    /// Last error message, if the task failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Total number of times the task has run.
    pub total_runs: u64,
    /// Total number of failures.
    pub total_failures: u64,
}

/// Registry of scheduled tasks and their runtime status.
#[derive(Clone)]
pub struct TaskRegistry {
    inner: Arc<RwLock<HashMap<String, TaskStatus>>>,
}

impl TaskRegistry {
    /// Create a new empty task registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a task with its schedule description.
    pub fn register(&self, name: &str, schedule: &str) {
        if let Ok(mut guard) = self.inner.write() {
            guard.insert(
                name.to_string(),
                TaskStatus {
                    schedule: schedule.to_string(),
                    status: "idle".to_string(),
                    last_run: None,
                    last_duration_ms: None,
                    last_result: None,
                    last_error: None,
                    total_runs: 0,
                    total_failures: 0,
                },
            );
        }
    }

    /// Record that a task started running.
    pub fn record_start(&self, name: &str) {
        if let Ok(mut guard) = self.inner.write() {
            if let Some(task) = guard.get_mut(name) {
                task.status = "running".to_string();
            }
        }
    }

    /// Record that a task completed successfully.
    pub fn record_success(&self, name: &str, duration_ms: u64) {
        if let Ok(mut guard) = self.inner.write() {
            if let Some(task) = guard.get_mut(name) {
                task.status = "idle".to_string();
                task.last_run = Some(chrono::Utc::now().to_rfc3339());
                task.last_duration_ms = Some(duration_ms);
                task.last_result = Some("ok".to_string());
                task.last_error = None;
                task.total_runs += 1;
            }
        }
    }

    /// Record that a task failed.
    pub fn record_failure(&self, name: &str, duration_ms: u64, error: &str) {
        if let Ok(mut guard) = self.inner.write() {
            if let Some(task) = guard.get_mut(name) {
                task.status = "idle".to_string();
                task.last_run = Some(chrono::Utc::now().to_rfc3339());
                task.last_duration_ms = Some(duration_ms);
                task.last_result = Some("failed".to_string());
                task.last_error = Some(error.to_string());
                task.total_runs += 1;
                task.total_failures += 1;
            }
        }
    }

    /// Get a snapshot of all task statuses.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, TaskStatus> {
        self.inner
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TaskRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRegistry")
            .field("count", &self.snapshot().len())
            .finish()
    }
}

/// Resolved config property with source provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigProperty {
    /// The resolved value (redacted if sensitive).
    pub value: serde_json::Value,
    /// Where the value came from.
    pub source: String,
}

/// Collection of resolved config properties with source tracking.
#[derive(Debug, Clone, Default)]
pub struct ConfigProperties {
    inner: Arc<RwLock<HashMap<String, ConfigProperty>>>,
}

impl ConfigProperties {
    /// Build config properties with source tracking from the loaded config.
    #[must_use]
    pub fn from_config(config: &crate::config::AutumnConfig) -> Self {
        let profile = config.profile.as_deref().unwrap_or("default");
        let defaults = crate::config::AutumnConfig::default();

        let mut props = HashMap::new();
        let profile_str = profile.to_string();

        // Server properties
        Self::track_property(
            &mut props,
            "server.host",
            &config.server.host,
            &defaults.server.host,
            &profile_str,
        );
        Self::track_property(
            &mut props,
            "server.port",
            &config.server.port.to_string(),
            &defaults.server.port.to_string(),
            &profile_str,
        );
        Self::track_property(
            &mut props,
            "server.shutdown_timeout_secs",
            &config.server.shutdown_timeout_secs.to_string(),
            &defaults.server.shutdown_timeout_secs.to_string(),
            &profile_str,
        );

        // Database properties
        let db_url = config.database.url.as_deref().unwrap_or("").to_string();
        Self::track_property(&mut props, "database.url", &db_url, "", &profile_str);
        Self::track_property(
            &mut props,
            "database.pool_size",
            &config.database.pool_size.to_string(),
            &defaults.database.pool_size.to_string(),
            &profile_str,
        );

        // Log properties
        Self::track_property(
            &mut props,
            "log.level",
            &config.log.level,
            &defaults.log.level,
            &profile_str,
        );
        Self::track_property(
            &mut props,
            "log.format",
            &format!("{:?}", config.log.format),
            &format!("{:?}", defaults.log.format),
            &profile_str,
        );

        // Health properties
        Self::track_property(
            &mut props,
            "health.path",
            &config.health.path,
            &defaults.health.path,
            &profile_str,
        );
        Self::track_property(
            &mut props,
            "health.detailed",
            &config.health.detailed.to_string(),
            &defaults.health.detailed.to_string(),
            &profile_str,
        );

        // Actuator properties
        Self::track_property(
            &mut props,
            "actuator.prefix",
            &config.actuator.prefix,
            &defaults.actuator.prefix,
            &profile_str,
        );
        Self::track_property(
            &mut props,
            "actuator.sensitive",
            &config.actuator.sensitive.to_string(),
            &defaults.actuator.sensitive.to_string(),
            &profile_str,
        );

        Self {
            inner: Arc::new(RwLock::new(props)),
        }
    }

    /// Track a single config property, determining its source by checking
    /// for env var overrides and comparing against defaults.
    fn track_property(
        props: &mut HashMap<String, ConfigProperty>,
        key: &str,
        value: &str,
        default_value: &str,
        profile: &str,
    ) {
        // Check if there's an env var override
        let env_key = format!("AUTUMN_{}", key.replace('.', "__").to_uppercase());
        let source = if std::env::var(&env_key).is_ok() {
            env_key
        } else if value != default_value && (profile == "dev" || profile == "prod") {
            format!("profile_default:{profile}")
        } else if value != default_value {
            "autumn.toml".to_string()
        } else {
            "default".to_string()
        };

        let display_value = if should_redact(key) {
            serde_json::Value::String("****".into())
        } else {
            serde_json::Value::String(value.to_string())
        };

        props.insert(
            key.to_string(),
            ConfigProperty {
                value: display_value,
                source,
            },
        );
    }

    /// Get a snapshot of all properties.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, ConfigProperty> {
        self.inner
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

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
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let mut db_check = None;
    let mut overall_healthy = true;

    #[cfg(feature = "db")]
    if let Some(pool) = state.pool.as_ref() {
        let status = pool.status();
        let available = status.available as u64;
        let size = status.max_size as u64;
        let waiting = status.waiting as u64;
        let idle = available;
        let active = size.saturating_sub(available);

        let healthy = available > 0 || waiting == 0;
        if !healthy {
            overall_healthy = false;
        }

        db_check = Some(DatabaseCheck {
            status: if healthy { "ok" } else { "down" },
            pool_size: size,
            active_connections: active,
            idle_connections: idle,
        });
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
pub(crate) async fn info(State(state): State<AppState>) -> Json<ActuatorInfo> {
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

/// Keys that trigger value redaction.
const REDACT_PATTERNS: &[&str] = &[
    "password",
    "secret",
    "key",
    "token",
    "credential",
    "auth",
    "url",
];

fn should_redact(key: &str) -> bool {
    let lower = key.to_lowercase();
    REDACT_PATTERNS.iter().any(|p| lower.contains(p))
}

/// `GET /actuator/env` — only available when actuator sensitive mode is enabled.
pub(crate) async fn env_endpoint(State(state): State<AppState>) -> Json<ActuatorEnv> {
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
            let val = if should_redact(k) {
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
pub(crate) async fn metrics_endpoint(State(state): State<AppState>) -> Json<serde_json::Value> {
    let snapshot = state.metrics.snapshot();
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
pub(crate) async fn configprops_endpoint(State(state): State<AppState>) -> Json<serde_json::Value> {
    let props = state.config_props.snapshot();

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
pub(crate) async fn loggers_get(State(state): State<AppState>) -> Json<LoggersResponse> {
    Json(LoggersResponse {
        current_level: state.log_levels.current_level(),
        available_levels: AVAILABLE_LEVELS.to_vec(),
        loggers: state.log_levels.logger_overrides(),
    })
}

/// Request body for `PUT /actuator/loggers/{name}`.
#[derive(Deserialize)]
pub(crate) struct SetLoggerRequest {
    level: String,
}

/// `PUT /actuator/loggers/{name}` -- change a logger's level at runtime.
pub(crate) async fn loggers_put(
    State(state): State<AppState>,
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

    let previous = state.log_levels.set_logger_level(&name, &level);

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
pub(crate) async fn tasks_endpoint(State(state): State<AppState>) -> Json<serde_json::Value> {
    let tasks = state.task_registry.snapshot();

    Json(serde_json::json!({
        "scheduled_tasks": tasks,
    }))
}

#[cfg(feature = "dashboard")]
/// `GET /actuator` -- HTML dashboard of system health, metrics, and tasks.
pub(crate) async fn dashboard_endpoint(State(state): State<AppState>) -> maud::Markup {
    use maud::{DOCTYPE, html};

    let uptime = state.uptime_display();
    let profile = state.profile();
    let metrics = state.metrics.snapshot();
    let tasks = state.task_registry.snapshot();

    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { "Autumn Actuator Dashboard" }
                style {
                    "body { font-family: system-ui, -apple-system, sans-serif; background-color: #f3f4f6; color: #111827; margin: 0; padding: 2rem; }"
                    "h1, h2 { color: #1f2937; }"
                    ".grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 1.5rem; }"
                    ".card { background: white; padding: 1.5rem; border-radius: 0.5rem; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }"
                    ".stat-value { font-size: 2rem; font-weight: bold; color: #4f46e5; }"
                    ".stat-label { font-size: 0.875rem; color: #6b7280; text-transform: uppercase; letter-spacing: 0.05em; margin-top: 0.5rem; }"
                    "table { width: 100%; border-collapse: collapse; margin-top: 1rem; }"
                    "th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #e5e7eb; }"
                    "th { font-weight: 600; color: #374151; }"
                }
            }
            body {
                h1 style="margin-bottom: 2rem;" { "🍂 Autumn Actuator Dashboard" }

                div class="grid" {
                    div class="card" {
                        div class="stat-value" { (profile) }
                        div class="stat-label" { "Active Profile" }
                    }
                    div class="card" {
                        div class="stat-value" { (uptime) }
                        div class="stat-label" { "Uptime" }
                    }
                    div class="card" {
                        div class="stat-value" { (metrics.http.requests_total) }
                        div class="stat-label" { "Total Requests" }
                    }
                    div class="card" {
                        div class="stat-value" { (metrics.http.requests_active) }
                        div class="stat-label" { "Active Requests" }
                    }
                }

                div class="grid" style="margin-top: 2rem;" {
                    div class="card" {
                        h2 { "HTTP Metrics" }
                        table {
                            tr { th { "Route" } th { "Count" } th { "p50 (ms)" } th { "p99 (ms)" } }
                            @for (route, stat) in &metrics.http.by_route {
                                tr {
                                    td { (route) }
                                    td { (stat.count) }
                                    td { (stat.p50_ms) }
                                    td { (stat.p99_ms) }
                                }
                            }
                            @if metrics.http.by_route.is_empty() {
                                tr { td colspan="4" style="text-align: center; color: #6b7280;" { "No request data yet" } }
                            }
                        }
                    }

                    div class="card" {
                        h2 { "Scheduled Tasks" }
                        table {
                            tr { th { "Task" } th { "Schedule" } th { "Total Runs" } th { "Status" } }
                            @for (name, status) in &tasks {
                                tr {
                                    td { (name) }
                                    td { (status.schedule) }
                                    td { (status.total_runs) }
                                    td {
                                        @if status.status == "running" {
                                            span style="color: #059669;" { "Running" }
                                        } @else {
                                            span style="color: #6b7280;" { (status.status) }
                                        }
                                    }
                                }
                            }
                            @if tasks.is_empty() {
                                tr { td colspan="4" style="text-align: center; color: #6b7280;" { "No tasks registered" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Router builder ──────────────────────────────────────────────

/// Build the actuator router with profile-aware endpoint exposure.
///
/// In dev mode (or when `actuator.sensitive = true`), all endpoints are
/// exposed. In prod mode, only health, info, and metrics are available.
pub fn actuator_router(sensitive: bool) -> axum::Router<AppState> {
    let mut router = axum::Router::new()
        .route("/actuator/health", axum::routing::get(health))
        .route("/actuator/info", axum::routing::get(info))
        .route("/actuator/metrics", axum::routing::get(metrics_endpoint));

    if sensitive {
        #[cfg(feature = "dashboard")]
        {
            router = router.route("/actuator", axum::routing::get(dashboard_endpoint));
        }

        router = router
            .route("/actuator/env", axum::routing::get(env_endpoint))
            .route(
                "/actuator/configprops",
                axum::routing::get(configprops_endpoint),
            )
            .route("/actuator/loggers", axum::routing::get(loggers_get))
            .route("/actuator/loggers/{name}", axum::routing::put(loggers_put))
            .route("/actuator/tasks", axum::routing::get(tasks_endpoint));
    }

    router
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
            log_levels: LogLevels::new("info"),
            task_registry: TaskRegistry::new(),
            config_props: ConfigProperties::default(),
        }
    }

    #[cfg(feature = "dashboard")]
    #[tokio::test]
    async fn actuator_dashboard_returns_html() {
        let app = actuator_router(true).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(body_str.contains("Autumn Actuator Dashboard"));
        assert!(body_str.contains("HTTP Metrics"));
    }

    #[cfg(feature = "dashboard")]
    #[tokio::test]
    async fn actuator_dashboard_hidden_in_nonsensitive_mode() {
        let app = actuator_router(false).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
        assert!(should_redact("database.url"));
        assert!(should_redact("api_token"));
        assert!(should_redact("secret_key"));
        assert!(!should_redact("server.port"));
        assert!(!should_redact("log.level"));
    }

    // ── Metrics endpoint tests ─────────────────────────────────

    #[tokio::test]
    async fn actuator_metrics_returns_http_stats() {
        let state = test_state();
        state.metrics.record("GET", "/test", 200, 10);
        state.metrics.record("POST", "/test", 500, 50);

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

        let overrides = state.log_levels.logger_overrides();
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
        state.task_registry.register("cleanup", "every 5m");
        state.task_registry.record_start("cleanup");
        state.task_registry.record_success("cleanup", 150);

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
}
