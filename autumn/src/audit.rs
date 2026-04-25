//! Structured audit logging with pluggable sinks.
//!
//! Audit logs are intentionally separate from regular application logs and
//! should capture security-sensitive, compliance-relevant actions.
//! Autumn models audit writes as append-only events sent to one or more
//! sinks (database, SIEM adapter, dedicated file, etc.).

use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::state::AppState;

/// Outcome of an audited action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    /// The action completed successfully.
    Success,
    /// The action was attempted but failed.
    Failure,
}

/// A structured audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEvent {
    /// UTC timestamp recorded when the event was created.
    pub timestamp: DateTime<Utc>,
    /// Actor identifier (user ID, service account ID, or API key ID).
    pub actor_id: String,
    /// Canonical action name (for example, `"user.role.update"`).
    pub action: String,
    /// Target resource identifier affected by the action.
    pub target_resource_id: String,
    /// Caller IP address if known.
    pub ip_address: Option<IpAddr>,
    /// Final action status.
    pub status: AuditStatus,
}

impl AuditEvent {
    /// Create a new audit event with the current UTC timestamp.
    #[must_use]
    pub fn new(
        actor_id: impl Into<String>,
        action: impl Into<String>,
        target_resource_id: impl Into<String>,
        ip_address: Option<IpAddr>,
        status: AuditStatus,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            actor_id: actor_id.into(),
            action: action.into(),
            target_resource_id: target_resource_id.into(),
            ip_address,
            status,
        }
    }
}

/// Error returned by audit sinks.
#[derive(Debug, Error)]
#[error("audit sink write failed: {message}")]
pub struct AuditError {
    message: String,
}

impl AuditError {
    /// Create a new sink error with a human-readable message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn message(&self) -> &str {
        &self.message
    }
}

type AuditWriteFuture<'a> = Pin<Box<dyn Future<Output = Result<(), AuditError>> + Send + 'a>>;

/// A destination for append-only audit events.
pub trait AuditSink: Send + Sync + 'static {
    /// Persist one audit event. Implementations must treat events as immutable,
    /// append-only records.
    fn write(&self, event: AuditEvent) -> AuditWriteFuture<'_>;
}

/// Shared audit writer that fans out to multiple sinks.
#[derive(Clone, Default)]
pub struct AuditLogger {
    sinks: Vec<Arc<dyn AuditSink>>,
}

impl AuditLogger {
    /// Create an empty logger with no sinks.
    #[must_use]
    pub const fn new() -> Self {
        Self { sinks: Vec::new() }
    }

    /// Register an audit sink.
    #[must_use]
    pub fn with_sink(mut self, sink: Arc<dyn AuditSink>) -> Self {
        self.sinks.push(sink);
        self
    }

    /// Append an event to all configured sinks.
    ///
    /// # Errors
    ///
    /// Returns [`AuditError`] when one or more configured sinks fail. All
    /// sinks are still attempted; failures are aggregated into one error.
    pub async fn write(&self, event: AuditEvent) -> Result<(), AuditError> {
        let mut errors = Vec::new();
        for sink in &self.sinks {
            if let Err(error) = sink.write(event.clone()).await {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            let details = errors
                .iter()
                .map(|error| error.message().to_owned())
                .collect::<Vec<_>>()
                .join(" | ");
            Err(AuditError::new(format!(
                "{} audit sink(s) failed: {details}",
                errors.len()
            )))
        }
    }

    /// Returns true when at least one sink is configured.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.sinks.is_empty()
    }
}

/// Tracing-based sink that emits JSON fields on a dedicated `autumn.audit` target.
#[derive(Debug, Default)]
pub struct TracingAuditSink;

impl AuditSink for TracingAuditSink {
    fn write(&self, event: AuditEvent) -> AuditWriteFuture<'_> {
        Box::pin(async move {
            tracing::info!(
                target: "autumn.audit",
                timestamp = %event.timestamp,
                actor_id = %event.actor_id,
                action = %event.action,
                target_resource_id = %event.target_resource_id,
                ip_address = ?event.ip_address,
                status = ?event.status,
                "audit_event"
            );
            Ok(())
        })
    }
}

/// JSON-lines file sink suitable for immutable append-only audit archives.
#[derive(Debug)]
pub struct JsonlFileAuditSink {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl JsonlFileAuditSink {
    /// Create a JSONL sink writing to `path` in append-only mode.
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            write_lock: Mutex::new(()),
        }
    }
}

impl AuditSink for JsonlFileAuditSink {
    fn write(&self, event: AuditEvent) -> AuditWriteFuture<'_> {
        Box::pin(async move {
            let mut encoded = serde_json::to_vec(&event).map_err(|error| {
                AuditError::new(format!("failed to encode audit event: {error}"))
            })?;
            encoded.push(b'\n');
            let _guard = self.write_lock.lock().await;
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .await
                .map_err(|error| {
                    AuditError::new(format!("failed to open audit log file: {error}"))
                })?;
            file.write_all(&encoded).await.map_err(|error| {
                AuditError::new(format!("failed to write audit event: {error}"))
            })?;
            Ok(())
        })
    }
}

/// Helper to write an audit event using the logger stored in [`AppState`].
///
/// # Errors
///
/// Returns [`AuditError`] when the installed logger fails to persist to one or
/// more sinks. If no logger is installed in state, this is a no-op and returns
/// `Ok(())`.
pub async fn write_from_state(state: &AppState, event: AuditEvent) -> Result<(), AuditError> {
    if let Some(logger) = state.extension::<AuditLogger>() {
        logger.write(event).await
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FailingSink;

    impl AuditSink for FailingSink {
        fn write(&self, _event: AuditEvent) -> AuditWriteFuture<'_> {
            Box::pin(async { Err(AuditError::new("boom")) })
        }
    }

    struct CountingSink {
        writes: Arc<AtomicUsize>,
    }

    impl AuditSink for CountingSink {
        fn write(&self, _event: AuditEvent) -> AuditWriteFuture<'_> {
            let writes = self.writes.clone();
            Box::pin(async move {
                writes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn jsonl_sink_appends_events() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("audit.log");
        let sink = JsonlFileAuditSink::new(&path);

        sink.write(AuditEvent::new(
            "admin-1",
            "user.role.update",
            "user-99",
            None,
            AuditStatus::Success,
        ))
        .await
        .expect("write first event");

        sink.write(AuditEvent::new(
            "api-key-1",
            "export.create",
            "export-42",
            None,
            AuditStatus::Failure,
        ))
        .await
        .expect("write second event");

        let content = tokio::fs::read_to_string(&path)
            .await
            .expect("read audit file");
        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 2, "content:\n{content}");
    }

    #[tokio::test]
    async fn write_from_state_no_logger_is_noop() {
        let state = AppState::for_test();
        write_from_state(
            &state,
            AuditEvent::new("u1", "auth.login", "session-1", None, AuditStatus::Success),
        )
        .await
        .expect("no-op write should succeed");
    }

    #[tokio::test]
    async fn audit_logger_continues_fan_out_after_sink_failure() {
        let writes = Arc::new(AtomicUsize::new(0));
        let logger = AuditLogger::new()
            .with_sink(Arc::new(FailingSink))
            .with_sink(Arc::new(CountingSink {
                writes: writes.clone(),
            }));

        let error = logger
            .write(AuditEvent::new(
                "u1",
                "auth.login",
                "session-1",
                None,
                AuditStatus::Failure,
            ))
            .await
            .expect_err("first sink should fail");

        assert!(
            error.to_string().contains("1 audit sink(s) failed"),
            "unexpected error: {error}"
        );
        assert_eq!(
            writes.load(Ordering::SeqCst),
            1,
            "second sink should still receive event"
        );
    }
}
