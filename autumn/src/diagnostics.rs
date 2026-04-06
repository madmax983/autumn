//! Diagnostics and telemetry types for the actuator endpoints.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

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
    pub fn track_property(
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

pub(crate) fn should_redact(key: &str) -> bool {
    let lower = key.to_lowercase();
    REDACT_PATTERNS.iter().any(|p| lower.contains(p))
}
