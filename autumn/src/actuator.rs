//! Actuator endpoints for operational observability.
//!
//! Provides health, info, env, metrics, configprops, loggers, and tasks
//! endpoints under the configured actuator prefix.
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

/// Trait to abstract the state requirements for actuator handlers.
///
/// Implement this trait on your application's state type to provide
/// the necessary dependencies for actuator endpoints (e.g. `/actuator/metrics`).
/// This avoids tight coupling between the actuator middleware and the specific `AppState`.
pub trait ProvideActuatorState {
    /// Returns a reference to the [`crate::middleware::MetricsCollector`]
    /// tracking current HTTP traffic metrics.
    fn metrics(&self) -> &crate::middleware::MetricsCollector;

    /// Returns a reference to the dynamic [`LogLevels`] configuration
    /// allowing runtime adjustment of `tracing` filters.
    fn log_levels(&self) -> &LogLevels;

    /// Returns a reference to the [`TaskRegistry`] holding status and metadata
    /// for async scheduled background tasks.
    fn task_registry(&self) -> &TaskRegistry;

    /// Returns a reference to the [`ConfigProperties`] snapshot, providing
    /// active configuration state for the environment endpoint.
    fn config_props(&self) -> &ConfigProperties;

    /// Returns the currently active execution profile (e.g. "dev", "prod")
    /// which modifies what sensitive endpoints are exposed.
    fn profile(&self) -> &str;

    /// Returns a human-readable string displaying how long the application
    /// has been running (e.g., "2d 4h 13m").
    fn uptime_display(&self) -> String;

    /// Returns a reference to the system [`crate::channels::Channels`] which
    /// broadcasts operational events to WebSocket streams.
    #[cfg(feature = "ws")]
    fn channels(&self) -> &crate::channels::Channels;

    /// Returns the main cancellation token that triggers a graceful framework shutdown.
    #[cfg(feature = "ws")]
    fn shutdown_token(&self) -> tokio_util::sync::CancellationToken;

    /// Returns an optional reference to the database connection pool,
    /// used to expose database connection metrics in the `/actuator/metrics` endpoint.
    #[cfg(feature = "db")]
    fn pool(
        &self,
    ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>;
}

// ── Shared types for AppState ──────────────────────────────────

/// Runtime log level management for the loggers actuator endpoint.
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
            // Prevent unbounded memory growth from arbitrary logger names
            if guard.logger_overrides.len() >= 1000 && !guard.logger_overrides.contains_key(name) {
                return None;
            }

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
    #[allow(clippy::too_many_lines)]
    pub fn from_config(config: &crate::config::AutumnConfig) -> Self {
        let profile = config.profile.as_deref().unwrap_or("default");
        let defaults = crate::config::AutumnConfig::default();

        // Avoids dynamic reallocation since we know roughly how many config properties are tracked.
        let mut props = HashMap::with_capacity(32);
        let profile_str = profile.to_string();

        Self::track_server_props(&mut props, config, &defaults, &profile_str);
        Self::track_db_props(&mut props, config, &defaults, &profile_str);
        Self::track_log_props(&mut props, config, &defaults, &profile_str);
        Self::track_telemetry_props(&mut props, config, &defaults, &profile_str);
        Self::track_health_props(&mut props, config, &defaults, &profile_str);
        Self::track_actuator_props(&mut props, config, &defaults, &profile_str);
        Self::track_session_props(&mut props, config, &defaults, &profile_str);

        Self {
            inner: Arc::new(RwLock::new(props)),
        }
    }

    fn track_server_props(
        props: &mut HashMap<String, ConfigProperty>,
        config: &crate::config::AutumnConfig,
        defaults: &crate::config::AutumnConfig,
        profile_str: &str,
    ) {
        Self::track_property(
            props,
            "server.host",
            &config.server.host,
            &defaults.server.host,
            profile_str,
        );
        Self::track_property(
            props,
            "server.port",
            &config.server.port.to_string(),
            &defaults.server.port.to_string(),
            profile_str,
        );
        Self::track_property(
            props,
            "server.shutdown_timeout_secs",
            &config.server.shutdown_timeout_secs.to_string(),
            &defaults.server.shutdown_timeout_secs.to_string(),
            profile_str,
        );
    }

    fn track_db_props(
        props: &mut HashMap<String, ConfigProperty>,
        config: &crate::config::AutumnConfig,
        defaults: &crate::config::AutumnConfig,
        profile_str: &str,
    ) {
        let db_url = config.database.url.as_deref().unwrap_or("").to_string();
        Self::track_property(props, "database.url", &db_url, "", profile_str);
        Self::track_property(
            props,
            "database.pool_size",
            &config.database.pool_size.to_string(),
            &defaults.database.pool_size.to_string(),
            profile_str,
        );
    }

    fn track_log_props(
        props: &mut HashMap<String, ConfigProperty>,
        config: &crate::config::AutumnConfig,
        defaults: &crate::config::AutumnConfig,
        profile_str: &str,
    ) {
        Self::track_property(
            props,
            "log.level",
            &config.log.level,
            &defaults.log.level,
            profile_str,
        );
        Self::track_property(
            props,
            "log.format",
            &format!("{:?}", config.log.format),
            &format!("{:?}", defaults.log.format),
            profile_str,
        );
    }

    fn track_telemetry_props(
        props: &mut HashMap<String, ConfigProperty>,
        config: &crate::config::AutumnConfig,
        defaults: &crate::config::AutumnConfig,
        profile_str: &str,
    ) {
        Self::track_property(
            props,
            "telemetry.enabled",
            &config.telemetry.enabled.to_string(),
            &defaults.telemetry.enabled.to_string(),
            profile_str,
        );
        Self::track_property(
            props,
            "telemetry.service_name",
            &config.telemetry.service_name,
            &defaults.telemetry.service_name,
            profile_str,
        );
        Self::track_property(
            props,
            "telemetry.service_namespace",
            config.telemetry.service_namespace.as_deref().unwrap_or(""),
            defaults
                .telemetry
                .service_namespace
                .as_deref()
                .unwrap_or(""),
            profile_str,
        );
        Self::track_property(
            props,
            "telemetry.service_version",
            &config.telemetry.service_version,
            &defaults.telemetry.service_version,
            profile_str,
        );
        Self::track_property(
            props,
            "telemetry.environment",
            &config.telemetry.environment,
            &defaults.telemetry.environment,
            profile_str,
        );
        Self::track_property(
            props,
            "telemetry.otlp_endpoint",
            config.telemetry.otlp_endpoint.as_deref().unwrap_or(""),
            defaults.telemetry.otlp_endpoint.as_deref().unwrap_or(""),
            profile_str,
        );
        Self::track_property(
            props,
            "telemetry.protocol",
            &format!("{:?}", config.telemetry.protocol),
            &format!("{:?}", defaults.telemetry.protocol),
            profile_str,
        );
        Self::track_property(
            props,
            "telemetry.strict",
            &config.telemetry.strict.to_string(),
            &defaults.telemetry.strict.to_string(),
            profile_str,
        );
    }

    fn track_health_props(
        props: &mut HashMap<String, ConfigProperty>,
        config: &crate::config::AutumnConfig,
        defaults: &crate::config::AutumnConfig,
        profile_str: &str,
    ) {
        Self::track_property(
            props,
            "health.path",
            &config.health.path,
            &defaults.health.path,
            profile_str,
        );
        Self::track_property(
            props,
            "health.live_path",
            &config.health.live_path,
            &defaults.health.live_path,
            profile_str,
        );
        Self::track_property(
            props,
            "health.ready_path",
            &config.health.ready_path,
            &defaults.health.ready_path,
            profile_str,
        );
        Self::track_property(
            props,
            "health.startup_path",
            &config.health.startup_path,
            &defaults.health.startup_path,
            profile_str,
        );
        Self::track_property(
            props,
            "health.detailed",
            &config.health.detailed.to_string(),
            &defaults.health.detailed.to_string(),
            profile_str,
        );
    }

    fn track_actuator_props(
        props: &mut HashMap<String, ConfigProperty>,
        config: &crate::config::AutumnConfig,
        defaults: &crate::config::AutumnConfig,
        profile_str: &str,
    ) {
        Self::track_property(
            props,
            "actuator.prefix",
            &config.actuator.prefix,
            &defaults.actuator.prefix,
            profile_str,
        );
        Self::track_property(
            props,
            "actuator.sensitive",
            &config.actuator.sensitive.to_string(),
            &defaults.actuator.sensitive.to_string(),
            profile_str,
        );
    }

    fn track_session_props(
        props: &mut HashMap<String, ConfigProperty>,
        config: &crate::config::AutumnConfig,
        defaults: &crate::config::AutumnConfig,
        profile_str: &str,
    ) {
        Self::track_property(
            props,
            "session.backend",
            &format!("{:?}", config.session.backend),
            &format!("{:?}", defaults.session.backend),
            profile_str,
        );
        Self::track_property(
            props,
            "session.cookie_name",
            &config.session.cookie_name,
            &defaults.session.cookie_name,
            profile_str,
        );
        Self::track_property(
            props,
            "session.max_age_secs",
            &config.session.max_age_secs.to_string(),
            &defaults.session.max_age_secs.to_string(),
            profile_str,
        );
        Self::track_property(
            props,
            "session.secure",
            &config.session.secure.to_string(),
            &defaults.session.secure.to_string(),
            profile_str,
        );
        Self::track_property(
            props,
            "session.same_site",
            &config.session.same_site,
            &defaults.session.same_site,
            profile_str,
        );
        Self::track_property(
            props,
            "session.http_only",
            &config.session.http_only.to_string(),
            &defaults.session.http_only.to_string(),
            profile_str,
        );
        Self::track_property(
            props,
            "session.path",
            &config.session.path,
            &defaults.session.path,
            profile_str,
        );
        Self::track_property(
            props,
            "session.allow_memory_in_production",
            &config.session.allow_memory_in_production.to_string(),
            &defaults.session.allow_memory_in_production.to_string(),
            profile_str,
        );
        Self::track_property(
            props,
            "session.redis.url",
            config.session.redis.url.as_deref().unwrap_or(""),
            defaults.session.redis.url.as_deref().unwrap_or(""),
            profile_str,
        );
        Self::track_property(
            props,
            "session.redis.key_prefix",
            &config.session.redis.key_prefix,
            &defaults.session.redis.key_prefix,
            profile_str,
        );
    }

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

/// `GET <actuator-prefix>/health`
pub async fn health<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> impl IntoResponse {
    let (overall_healthy, db_check) = {
        #[cfg(feature = "db")]
        {
            #[allow(clippy::option_if_let_else)]
            if let Some(pool) = state.pool() {
                let status = pool.status();
                let available = status.available as u64;
                let size = status.max_size as u64;
                let waiting = status.waiting as u64;
                let idle = available;
                let active = size.saturating_sub(available);

                let overall_healthy = available > 0 || waiting == 0;
                let db_check = Some(DatabaseCheck {
                    status: if overall_healthy { "ok" } else { "down" },
                    pool_size: size,
                    active_connections: active,
                    idle_connections: idle,
                });
                (overall_healthy, db_check)
            } else {
                (true, None)
            }
        }

        #[cfg(not(feature = "db"))]
        {
            (true, None)
        }
    };

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

/// `GET <actuator-prefix>/info`
pub(crate) async fn info<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> Json<ActuatorInfo> {
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
pub(crate) async fn env_endpoint<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> Json<ActuatorEnv> {
    let properties = state
        .config_props()
        .snapshot()
        .into_iter()
        .map(|(key, prop)| (key, prop.value))
        .collect();

    Json(ActuatorEnv {
        active_profile: state.profile().to_owned(),
        properties,
    })
}

// ── Metrics ────────────────────────────────────────────────────

/// `GET <actuator-prefix>/metrics` -- request metrics, latency, status codes, DB pool stats.
pub(crate) async fn metrics_endpoint<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> Json<serde_json::Value> {
    let snapshot = state.metrics().snapshot();
    let result = serde_json::to_value(&snapshot).unwrap_or_default();

    // Include DB pool stats if available
    #[cfg(feature = "db")]
    let result = {
        let mut result = result;
        if let Some(pool) = state.pool() {
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
        result
    };

    Json(result)
}

// ── Prometheus ─────────────────────────────────────────────────

/// `GET <actuator-prefix>/prometheus` -- export metrics in Prometheus format.
pub(crate) async fn prometheus_endpoint<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> impl IntoResponse {
    use std::fmt::Write;

    let snapshot = state.metrics().snapshot();
    let mut out = String::with_capacity(1024);

    // requests_total
    out.push_str("# HELP autumn_http_requests_total Total number of HTTP requests\n");
    out.push_str("# TYPE autumn_http_requests_total counter\n");
    let _ = writeln!(
        out,
        "autumn_http_requests_total {}",
        snapshot.http.requests_total
    );

    // requests_active
    out.push_str("# HELP autumn_http_requests_active Currently active HTTP requests\n");
    out.push_str("# TYPE autumn_http_requests_active gauge\n");
    let _ = writeln!(
        out,
        "autumn_http_requests_active {}",
        snapshot.http.requests_active
    );

    // by_status
    out.push_str("# HELP autumn_http_responses_total HTTP responses by status code\n");
    out.push_str("# TYPE autumn_http_responses_total counter\n");
    let _ = writeln!(
        out,
        "autumn_http_responses_total{{status=\"2xx\"}} {}",
        snapshot.http.by_status.s2xx
    );
    let _ = writeln!(
        out,
        "autumn_http_responses_total{{status=\"3xx\"}} {}",
        snapshot.http.by_status.s3xx
    );
    let _ = writeln!(
        out,
        "autumn_http_responses_total{{status=\"4xx\"}} {}",
        snapshot.http.by_status.s4xx
    );
    let _ = writeln!(
        out,
        "autumn_http_responses_total{{status=\"5xx\"}} {}",
        snapshot.http.by_status.s5xx
    );

    // by_route
    if !snapshot.http.by_route.is_empty() {
        out.push_str("# HELP autumn_http_route_requests_total HTTP requests by route and method\n");
        out.push_str("# TYPE autumn_http_route_requests_total counter\n");
        for (route_key, metrics) in &snapshot.http.by_route {
            // route_key is formatted as "METHOD /path"
            if let Some((method, path)) = route_key.split_once(' ') {
                let _ = writeln!(
                    out,
                    "autumn_http_route_requests_total{{method=\"{}\",route=\"{}\"}} {}",
                    method, path, metrics.count
                );
            }
        }
    }

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
}

// ── Config Properties (sensitive) ──────────────────────────────

/// `GET <actuator-prefix>/configprops` -- all config properties with source tracking.
pub(crate) async fn configprops_endpoint<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
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

/// Response for `GET <actuator-prefix>/loggers`.
#[derive(Serialize)]
pub(crate) struct LoggersResponse {
    current_level: String,
    available_levels: Vec<&'static str>,
    loggers: HashMap<String, String>,
}

/// `GET <actuator-prefix>/loggers` -- view current log levels.
pub(crate) async fn loggers_get<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> Json<LoggersResponse> {
    Json(LoggersResponse {
        current_level: state.log_levels().current_level(),
        available_levels: AVAILABLE_LEVELS.to_vec(),
        loggers: state.log_levels().logger_overrides(),
    })
}

/// Request body for `PUT <actuator-prefix>/loggers/{name}`.
#[derive(Deserialize)]
pub(crate) struct SetLoggerRequest {
    level: String,
}

/// `PUT <actuator-prefix>/loggers/{name}` -- change a logger's level at runtime.
pub(crate) async fn loggers_put<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
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

/// `GET <actuator-prefix>/tasks` -- scheduled task status.
pub(crate) async fn tasks_endpoint<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> Json<serde_json::Value> {
    let tasks = state.task_registry().snapshot();

    Json(serde_json::json!({
        "scheduled_tasks": tasks,
    }))
}

// ── Channels (sensitive) ───────────────────────────────────────

/// `GET <actuator-prefix>/channels` -- get current channel snapshots.
#[cfg(feature = "ws")]
pub(crate) async fn channels_endpoint<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
) -> Json<serde_json::Value> {
    let channels = state.channels().snapshot();
    Json(serde_json::json!({
        "channels": channels,
    }))
}

// ── Tasks Stream (WebSocket) ───────────────────────────────────

/// `GET <actuator-prefix>/tasks/stream` -- stream scheduled task events.
#[cfg(feature = "ws")]
pub(crate) async fn tasks_stream_endpoint<S: ProvideActuatorState + Send + Sync + 'static>(
    State(state): State<S>,
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

pub(crate) fn normalize_actuator_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim();
    if trimmed.is_empty() || trimmed == "/" {
        String::new()
    } else {
        let trimmed = trimmed.trim_end_matches('/');
        if trimmed.starts_with('/') {
            trimmed.to_owned()
        } else {
            format!("/{trimmed}")
        }
    }
}

pub(crate) fn actuator_route_glob(prefix: &str) -> String {
    let prefix = normalize_actuator_prefix(prefix);
    if prefix.is_empty() {
        "/*".to_owned()
    } else {
        format!("{prefix}/*")
    }
}

pub(crate) fn actuator_route_path(prefix: &str, suffix: &str) -> String {
    let prefix = normalize_actuator_prefix(prefix);
    if prefix.is_empty() {
        suffix.to_owned()
    } else {
        format!("{prefix}{suffix}")
    }
}

pub(crate) fn actuator_endpoint_paths(prefix: &str, sensitive: bool) -> Vec<String> {
    let mut paths = vec![
        actuator_route_path(prefix, "/health"),
        actuator_route_path(prefix, "/info"),
        actuator_route_path(prefix, "/metrics"),
        actuator_route_path(prefix, "/ui"),
        actuator_route_path(prefix, "/ui/metrics"),
    ];

    if sensitive {
        paths.push(actuator_route_path(prefix, "/env"));
        paths.push(actuator_route_path(prefix, "/configprops"));
        paths.push(actuator_route_path(prefix, "/loggers"));
        paths.push(actuator_route_path(prefix, "/tasks"));
        paths.push(actuator_route_path(prefix, "/ui/tasks"));
        paths.push(actuator_route_path(prefix, "/prometheus"));
        #[cfg(feature = "ws")]
        {
            paths.push(actuator_route_path(prefix, "/channels"));
            paths.push(actuator_route_path(prefix, "/tasks/stream"));
        }
    }

    paths
}

/// Build the actuator router with profile-aware endpoint exposure.
///
/// In dev mode (or when `actuator.sensitive = true`), all endpoints are
/// exposed. In prod mode, only health, info, and metrics are available.
pub fn actuator_router<S: ProvideActuatorState + Send + Sync + Clone + 'static>(
    sensitive: bool,
) -> axum::Router<S> {
    actuator_router_with_prefix("/actuator", sensitive)
}

/// Build the actuator router at a configured prefix.
///
/// This is the prefix-aware variant used by the framework router.
pub(crate) fn actuator_router_with_prefix<
    S: ProvideActuatorState + Send + Sync + Clone + 'static,
>(
    prefix: &str,
    sensitive: bool,
) -> axum::Router<S> {
    let mut router = axum::Router::new()
        .route(
            &actuator_route_path(prefix, "/health"),
            axum::routing::get(health::<S>),
        )
        .route(
            &actuator_route_path(prefix, "/info"),
            axum::routing::get(info::<S>),
        )
        .route(
            &actuator_route_path(prefix, "/metrics"),
            axum::routing::get(metrics_endpoint::<S>),
        );

    if sensitive {
        router = router
            .route(
                &actuator_route_path(prefix, "/env"),
                axum::routing::get(env_endpoint::<S>),
            )
            .route(
                &actuator_route_path(prefix, "/prometheus"),
                axum::routing::get(prometheus_endpoint::<S>),
            )
            .route(
                &actuator_route_path(prefix, "/configprops"),
                axum::routing::get(configprops_endpoint::<S>),
            )
            .route(
                &actuator_route_path(prefix, "/loggers"),
                axum::routing::get(loggers_get::<S>),
            )
            .route(
                &actuator_route_path(prefix, "/loggers/{name}"),
                axum::routing::put(loggers_put::<S>),
            )
            .route(
                &actuator_route_path(prefix, "/tasks"),
                axum::routing::get(tasks_endpoint::<S>),
            )
            .route(
                &actuator_route_path(prefix, "/ui/tasks"),
                axum::routing::get(ui_tasks::<S>),
            );

        #[cfg(feature = "ws")]
        {
            router = router
                .route(
                    &actuator_route_path(prefix, "/channels"),
                    axum::routing::get(channels_endpoint::<S>),
                )
                .route(
                    &actuator_route_path(prefix, "/tasks/stream"),
                    axum::routing::get(tasks_stream_endpoint::<S>),
                );
        }
    }

    // Nova: Add HTMX UI endpoints available unconditionally like metrics
    router
        .route(
            &actuator_route_path(prefix, "/ui"),
            axum::routing::get(ui_dashboard),
        )
        .route(
            &actuator_route_path(prefix, "/ui/metrics"),
            axum::routing::get(ui_metrics::<S>),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutumnConfig;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[derive(Clone)]
    struct MockActuatorState {
        profile: String,
        metrics: crate::middleware::MetricsCollector,
        log_levels: LogLevels,
        task_registry: TaskRegistry,
        config_props: ConfigProperties,
        probes: crate::probe::ProbeState,
        health_detailed: bool,
        #[cfg(feature = "ws")]
        channels: crate::channels::Channels,
        #[cfg(feature = "ws")]
        shutdown_token: tokio_util::sync::CancellationToken,
        #[cfg(feature = "db")]
        pool: Option<
            diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        >,
    }

    impl ProvideActuatorState for MockActuatorState {
        fn metrics(&self) -> &crate::middleware::MetricsCollector {
            &self.metrics
        }
        fn log_levels(&self) -> &LogLevels {
            &self.log_levels
        }
        fn task_registry(&self) -> &TaskRegistry {
            &self.task_registry
        }
        fn config_props(&self) -> &ConfigProperties {
            &self.config_props
        }
        fn profile(&self) -> &str {
            &self.profile
        }
        fn uptime_display(&self) -> String {
            "0s".to_string()
        }
        #[cfg(feature = "ws")]
        fn channels(&self) -> &crate::channels::Channels {
            &self.channels
        }
        #[cfg(feature = "ws")]
        fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
            self.shutdown_token.clone()
        }
        #[cfg(feature = "db")]
        fn pool(
            &self,
        ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
        {
            self.pool.as_ref()
        }
    }

    impl crate::probe::ProvideProbeState for MockActuatorState {
        fn probes(&self) -> &crate::probe::ProbeState {
            &self.probes
        }
        fn health_detailed(&self) -> bool {
            self.health_detailed
        }
        fn profile(&self) -> &str {
            &self.profile
        }
        fn uptime_display(&self) -> String {
            "0s".to_string()
        }
        #[cfg(feature = "db")]
        fn pool(
            &self,
        ) -> Option<&diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>>
        {
            self.pool.as_ref()
        }
    }

    #[test]
    fn mock_actuator_state_coverage() {
        let state = test_state();
        let _ = state.metrics();
        let _ = state.log_levels();
        let _ = state.task_registry();
        let _ = state.config_props();
        let _ = ProvideActuatorState::profile(&state);
        let _ = ProvideActuatorState::uptime_display(&state);
        let _ = crate::probe::ProvideProbeState::profile(&state);
        let _ = crate::probe::ProvideProbeState::uptime_display(&state);
        let _ = crate::probe::ProvideProbeState::probes(&state);
        let _ = crate::probe::ProvideProbeState::health_detailed(&state);

        #[cfg(feature = "ws")]
        {
            let _ = state.channels();
            let _ = state.shutdown_token();
        }
        #[cfg(feature = "db")]
        {
            let _ = ProvideActuatorState::pool(&state);
            let _ = crate::probe::ProvideProbeState::pool(&state);
        }
    }

    fn test_state() -> MockActuatorState {
        test_state_with_config(&AutumnConfig::default())
    }

    fn test_state_with_config(config: &AutumnConfig) -> MockActuatorState {
        MockActuatorState {
            profile: config.profile.clone().unwrap_or_else(|| "dev".into()),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: LogLevels::new("info"),
            task_registry: TaskRegistry::new(),
            config_props: ConfigProperties::from_config(config),
            probes: crate::probe::ProbeState::ready_for_test(),
            health_detailed: true,
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown_token: tokio_util::sync::CancellationToken::new(),
            #[cfg(feature = "db")]
            pool: None,
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
    async fn actuator_routes_respect_custom_prefix() {
        let app = actuator_router_with_prefix("/ops", true).with_state(test_state());

        let prefixed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ops/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(prefixed.status(), StatusCode::OK);

        let legacy = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn actuator_route_helpers_normalize_prefixes() {
        assert_eq!(actuator_route_glob("ops/"), "/ops/*");
        assert_eq!(actuator_route_path("ops/", "/health"), "/ops/health");
        assert_eq!(actuator_route_glob("/"), "/*");
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
        let config = AutumnConfig {
            profile: Some("prod".into()),
            server: crate::config::ServerConfig {
                port: 4100,
                ..crate::config::ServerConfig::default()
            },
            telemetry: crate::config::TelemetryConfig {
                enabled: true,
                service_name: "cloud-app".into(),
                ..crate::config::TelemetryConfig::default()
            },
            health: crate::config::HealthConfig {
                path: "/healthz".into(),
                ..crate::config::HealthConfig::default()
            },
            ..AutumnConfig::default()
        };

        let app = actuator_router(true).with_state(test_state_with_config(&config));
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["active_profile"], "prod");
        assert_eq!(json["properties"]["server.port"], "4100");
        assert_eq!(json["properties"]["telemetry.enabled"], "true");
        assert_eq!(json["properties"]["telemetry.service_name"], "cloud-app");
        assert_eq!(json["properties"]["health.path"], "/healthz");
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

    #[tokio::test]
    #[cfg(feature = "db")]
    async fn actuator_metrics_returns_db_stats_when_pool_present() {
        use diesel_async::AsyncPgConnection;
        use diesel_async::pooled_connection::AsyncDieselConnectionManager;
        use diesel_async::pooled_connection::deadpool::Pool;

        let mut state = test_state();

        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(
            "postgres://postgres:postgres@localhost:5432/postgres",
        );
        let pool = Pool::builder(manager).build().unwrap();

        state.pool = Some(pool);

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

        assert!(json.get("database").is_some());
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

    // ── Prometheus endpoint tests ──────────────────────────────

    #[tokio::test]
    async fn actuator_prometheus_returns_metrics() {
        let state = test_state();
        state.metrics().record("GET", "/test", 200, 10);
        state.metrics().record("POST", "/test", 500, 50);

        let app = actuator_router(true).with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/prometheus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/plain; version=0.0.4"
        );

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();

        assert!(text.contains("# HELP autumn_http_requests_total Total number of HTTP requests"));
        assert!(text.contains("# TYPE autumn_http_requests_total counter"));
        assert!(text.contains("autumn_http_requests_total 2"));

        assert!(text.contains("autumn_http_requests_active "));
        assert!(text.contains("autumn_http_responses_total{status=\"2xx\"} 1"));
        assert!(text.contains("autumn_http_responses_total{status=\"5xx\"} 1"));

        assert!(
            text.contains("autumn_http_route_requests_total{method=\"GET\",route=\"/test\"} 1")
        );
        assert!(
            text.contains("autumn_http_route_requests_total{method=\"POST\",route=\"/test\"} 1")
        );
    }

    #[tokio::test]
    async fn actuator_prometheus_hidden_in_nonsensitive_mode() {
        let app = actuator_router(false).with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/prometheus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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

    #[tokio::test]
    async fn actuator_ui_dashboard_returns_html_or_unimplemented() {
        let app = actuator_router(true).with_state(test_state());

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/ui")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        if cfg!(feature = "maud") {
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(
                res.headers().get("content-type").unwrap(),
                "text/html; charset=utf-8"
            );
        } else {
            assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
        }
    }

    #[tokio::test]
    async fn actuator_ui_metrics_returns_html_or_unimplemented() {
        let app = actuator_router(true).with_state(test_state());

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/ui/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        if cfg!(feature = "maud") {
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(
                res.headers().get("content-type").unwrap(),
                "text/html; charset=utf-8"
            );
        } else {
            assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
        }
    }

    #[tokio::test]
    async fn actuator_ui_tasks_returns_html_or_unimplemented() {
        let app = actuator_router(true).with_state(test_state());

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/actuator/ui/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        if cfg!(feature = "maud") {
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(
                res.headers().get("content-type").unwrap(),
                "text/html; charset=utf-8"
            );
        } else {
            assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
        }
    }
}

#[cfg(test)]
mod havoc_proptest {
    use super::*;
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

// ── Nova: Actuator HTMX Dashboard UI ──────────────────────────

#[cfg(all(feature = "maud", feature = "htmx"))]
async fn ui_dashboard() -> impl IntoResponse {
    let html = maud::html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Autumn Actuator Dashboard" }
                script src="/static/js/htmx.min.js" {}
                style {
                    "body { font-family: system-ui, sans-serif; background: #f9fafb; color: #111827; margin: 0; padding: 2rem; }"
                    "h1 { font-size: 1.5rem; font-weight: 600; margin-bottom: 1.5rem; }"
                    ".grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 1.5rem; }"
                    ".card { background: white; padding: 1.5rem; border-radius: 0.5rem; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }"
                    ".card h2 { font-size: 1.125rem; font-weight: 500; margin-top: 0; margin-bottom: 1rem; border-bottom: 1px solid #e5e7eb; padding-bottom: 0.5rem; }"
                    ".stat { display: flex; justify-content: space-between; margin-bottom: 0.5rem; }"
                    ".stat-label { color: #6b7280; }"
                    ".stat-value { font-weight: 500; }"
                    ".task-item { border: 1px solid #e5e7eb; padding: 0.75rem; border-radius: 0.375rem; margin-bottom: 0.75rem; }"
                    ".task-name { font-weight: 600; display: block; margin-bottom: 0.25rem; }"
                    ".task-meta { font-size: 0.875rem; color: #6b7280; }"
                    ".badge { display: inline-block; padding: 0.125rem 0.375rem; border-radius: 9999px; font-size: 0.75rem; font-weight: 500; }"
                    ".badge-green { background: #dcfce7; color: #166534; }"
                    ".badge-gray { background: #f3f4f6; color: #374151; }"
                    ".badge-red { background: #fee2e2; color: #991b1b; }"
                }
            }
            body {
                h1 { "🍂 Autumn Actuator Dashboard" }
                div class="grid" {
                    div class="card" hx-get="ui/metrics" hx-trigger="load, every 2s" {
                        "Loading metrics..."
                    }
                    div class="card" hx-get="ui/tasks" hx-trigger="load, every 2s" {
                        "Loading tasks..."
                    }
                }
            }
        }
    };
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html.into_string(),
    )
}

#[cfg(not(all(feature = "maud", feature = "htmx")))]
async fn ui_dashboard() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Maud feature is required for the UI dashboard",
    )
}

#[cfg(all(feature = "maud", feature = "htmx"))]
async fn ui_metrics<S: ProvideActuatorState>(State(state): State<S>) -> impl IntoResponse {
    let metrics = state.metrics().snapshot();
    let uptime = state.uptime_display();

    let html = maud::html! {
        h2 { "System Metrics" }
        div class="stat" {
            span class="stat-label" { "Uptime" }
            span class="stat-value" { (uptime) }
        }
        div class="stat" {
            span class="stat-label" { "Total Requests" }
            span class="stat-value" { (metrics.http.requests_total) }
        }
        div class="stat" {
            span class="stat-label" { "Active Requests" }
            span class="stat-value" { (metrics.http.requests_active) }
        }
        div class="stat" {
            span class="stat-label" { "P95 Latency" }
            span class="stat-value" { (metrics.http.latency_ms.p95) " ms" }
        }
        div class="stat" {
            span class="stat-label" { "P99 Latency" }
            span class="stat-value" { (metrics.http.latency_ms.p99) " ms" }
        }
    };
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html.into_string(),
    )
}

#[cfg(not(all(feature = "maud", feature = "htmx")))]
async fn ui_metrics<S: ProvideActuatorState>() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Maud feature is required for the UI dashboard",
    )
}

#[cfg(all(feature = "maud", feature = "htmx"))]
async fn ui_tasks<S: ProvideActuatorState>(State(state): State<S>) -> impl IntoResponse {
    let tasks = state.task_registry().snapshot();

    let html = maud::html! {
        h2 { "Background Tasks" }
        @if tasks.is_empty() {
            p class="stat-label" { "No tasks registered." }
        } @else {
            @for (name, task) in tasks.iter() {
                div class="task-item" {
                    span class="task-name" { (name) }
                    div class="task-meta" {
                        @if task.status == "running" {
                            span class="badge badge-green" { "Running" }
                        } @else {
                            span class="badge badge-gray" { "Idle" }
                        }
                        " "
                        "Runs: " (task.total_runs)
                        @if task.total_failures > 0 {
                            " " span class="badge badge-red" { "Failures: " (task.total_failures) }
                        }
                    }
                }
            }
        }
    };
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html.into_string(),
    )
}

#[cfg(not(all(feature = "maud", feature = "htmx")))]
async fn ui_tasks<S: ProvideActuatorState>() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Maud feature is required for the UI dashboard",
    )
}
