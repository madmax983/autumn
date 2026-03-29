//! Diesel model structs mapping to harvest_* tables.
//!
//! Each model has two variants:
//! - The full struct (Queryable + Selectable) for reads
//! - A `New*` struct (Insertable) for inserts — only the fields we supply,
//!   letting Postgres fill in defaults.

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

use crate::schema::*;

// ── WorkflowExecution ─────────────────────────────────────────────

#[derive(Debug, Clone, Queryable, Selectable, Identifiable)]
#[diesel(table_name = harvest_workflow_executions)]
pub struct WorkflowExecution {
    pub id: Uuid,
    pub workflow_name: String,
    pub workflow_id: String,
    pub run_id: Uuid,
    pub shard_id: i32,
    pub state: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub parent_id: Option<Uuid>,
    pub sticky_worker_id: Option<String>,
    pub queue_name: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub execution_timeout: Option<chrono::Duration>,
    pub memo: Option<serde_json::Value>,
    pub search_attrs: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = harvest_workflow_executions)]
pub struct NewWorkflowExecution<'a> {
    pub id: Uuid,
    pub workflow_name: &'a str,
    pub workflow_id: &'a str,
    pub run_id: Uuid,
    pub shard_id: i32,
    pub input: serde_json::Value,
    pub queue_name: &'a str,
    pub execution_timeout: Option<chrono::Duration>,
    pub memo: Option<serde_json::Value>,
    pub search_attrs: Option<serde_json::Value>,
}

// ── HarvestEvent ─────────────────────────────────────────────────

#[derive(Debug, Clone, Queryable, Selectable, Identifiable)]
#[diesel(table_name = harvest_events)]
pub struct HarvestEvent {
    pub id: i64,
    pub workflow_exec_id: Uuid,
    pub event_id: i32,
    pub event_type: String,
    pub event_data: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = harvest_events)]
pub struct NewHarvestEvent<'a> {
    pub workflow_exec_id: Uuid,
    pub event_id: i32,
    pub event_type: &'a str,
    pub event_data: serde_json::Value,
}

// ── TaskQueue ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Queryable, Selectable, Identifiable)]
#[diesel(table_name = harvest_task_queue)]
pub struct TaskQueueItem {
    pub id: Uuid,
    pub queue_name: String,
    pub task_type: String,
    pub workflow_exec_id: Option<Uuid>,
    pub activity_name: Option<String>,
    pub input: serde_json::Value,
    pub state: String,
    pub priority: i32,
    pub worker_id: Option<String>,
    pub attempt: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub heartbeat_timeout: Option<chrono::Duration>,
    pub start_to_close: Option<chrono::Duration>,
    pub schedule_to_start: Option<chrono::Duration>,
    pub retry_policy: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = harvest_task_queue)]
pub struct NewTaskQueueItem<'a> {
    pub id: Uuid,
    pub queue_name: &'a str,
    pub task_type: &'a str,
    pub workflow_exec_id: Option<Uuid>,
    pub activity_name: Option<&'a str>,
    pub input: serde_json::Value,
    pub priority: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub heartbeat_timeout: Option<chrono::Duration>,
    pub start_to_close: Option<chrono::Duration>,
    pub schedule_to_start: Option<chrono::Duration>,
    pub retry_policy: Option<serde_json::Value>,
}
