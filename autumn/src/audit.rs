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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
}

type AuditWriteFuture<'a> = Pin<Box<dyn Future<Output = Result<(), AuditError>> + Send + 'a>>;

/// A destination for append-only audit events.
pub trait AuditSink: Send + Sync + 'static {
    /// Persist one audit event. Implementations must treat events as immutable,
    /// append-only records.
    fn write<'a>(&'a self, event: AuditEvent) -> AuditWriteFuture<'a>;
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
    pub async fn write(&self, event: AuditEvent) -> Result<(), AuditError> {
        for sink in &self.sinks {
            sink.write(event.clone()).await?;
        }
        Ok(())
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
    fn write<'a>(&'a self, event: AuditEvent) -> AuditWriteFuture<'a> {
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
}

impl JsonlFileAuditSink {
    /// Create a JSONL sink writing to `path` in append-only mode.
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
}

impl AuditSink for JsonlFileAuditSink {
    fn write<'a>(&'a self, event: AuditEvent) -> AuditWriteFuture<'a> {
        Box::pin(async move {
            let encoded = serde_json::to_vec(&event).map_err(|error| {
                AuditError::new(format!("failed to encode audit event: {error}"))
            })?;
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
            file.write_all(b"\n").await.map_err(|error| {
                AuditError::new(format!("failed to flush audit newline: {error}"))
            })?;
            Ok(())
        })
    }
}

/// Helper to write an audit event using the logger stored in [`AppState`].
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
        assert_eq!(content.lines().count(), 2);
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
}
