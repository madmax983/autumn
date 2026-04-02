//! Durable workflows powered by autumn-harvest.
//!
//! This module showcases the autumn-harvest workflow engine patterns
//! used in a Reddit clone. autumn-harvest lives in its own workspace
//! (`autumn-harvest/`) and provides durable, event-sourced workflow
//! orchestration with activities, signals, timers, and DAG scheduling.
//!
//! # How to use with autumn-harvest
//!
//! Add to your Cargo.toml (in the autumn-harvest workspace):
//!
//! ```toml
//! [dependencies]
//! autumn-harvest = { path = "../autumn-harvest" }
//! ```
//!
//! Then define workflows and activities:
//!
//! ```rust,ignore
//! use autumn_harvest::prelude::*;
//! use std::time::Duration;
//!
//! // ── Workflows ──────────────────────────────────────────────
//!
//! /// Orchestrate new user onboarding: subscribe to defaults,
//! /// send welcome message, seed initial karma.
//! #[workflow]
//! async fn user_onboarding(
//!     ctx: &WorkflowContext,
//!     input: serde_json::Value,
//! ) -> HarvestResult<serde_json::Value> {
//!     let user_id = input["user_id"].as_i64().unwrap();
//!     let username = input["username"].as_str().unwrap_or("unknown");
//!
//!     if !ctx.is_replaying() {
//!         tracing::info!(user_id, %username, "Starting onboarding");
//!     }
//!
//!     Ok(serde_json::json!({ "status": "onboarded" }))
//! }
//!
//! /// Orchestrate post moderation checks.
//! #[workflow]
//! async fn post_moderation(
//!     ctx: &WorkflowContext,
//!     input: serde_json::Value,
//! ) -> HarvestResult<serde_json::Value> {
//!     let post_id = input["post_id"].as_i64().unwrap();
//!
//!     if !ctx.is_replaying() {
//!         tracing::info!(post_id, "Starting moderation");
//!     }
//!
//!     Ok(serde_json::json!({ "moderation": "approved" }))
//! }
//!
//! // ── Activities ─────────────────────────────────────────────
//!
//! #[activity(
//!     start_to_close = "10s",
//!     retry = RetryPolicy::fixed(3, Duration::from_secs(2))
//! )]
//! async fn subscribe_to_defaults(
//!     _ctx: &ActivityContext,
//!     input: serde_json::Value,
//! ) -> HarvestResult<serde_json::Value> {
//!     Ok(serde_json::json!({ "subscribed": true }))
//! }
//!
//! #[activity(
//!     start_to_close = "30s",
//!     retry = RetryPolicy::exponential(3, Duration::from_secs(1))
//! )]
//! async fn send_welcome_message(
//!     _ctx: &ActivityContext,
//!     input: serde_json::Value,
//! ) -> HarvestResult<serde_json::Value> {
//!     Ok(serde_json::json!({ "message_sent": true }))
//! }
//!
//! #[activity(
//!     start_to_close = "15s",
//!     heartbeat_timeout = "5s",
//!     retry = RetryPolicy::fixed(2, Duration::from_secs(1))
//! )]
//! async fn check_content(
//!     ctx: &ActivityContext,
//!     input: serde_json::Value,
//! ) -> HarvestResult<serde_json::Value> {
//!     ctx.heartbeat()?;
//!     Ok(serde_json::json!({ "approved": true }))
//! }
//!
//! // ── DAG (scheduled pipeline) ──────────────────────────────
//!
//! #[dag(
//!     schedule = "0 */15 * * * *",
//!     catchup = false,
//!     max_active_runs = 1,
//!     default_queue = "ranking"
//! )]
//! fn hot_rank_pipeline(dag: &mut DagBuilder) {
//!     let snapshot = dag.activity(snapshot_scores);
//!     let _recalc = dag
//!         .activity(recalculate_ranks)
//!         .upstream(&snapshot)
//!         .trigger_rule(TriggerRule::AllSuccess);
//! }
//!
//! // ── Engine setup ──────────────────────────────────────────
//!
//! fn harvest_engine() -> HarvestBuilder {
//!     HarvestBuilder::new()
//!         .workflows(workflows![user_onboarding, post_moderation])
//!         .activities(activities![
//!             subscribe_to_defaults,
//!             send_welcome_message,
//!             check_content,
//!         ])
//!         .dags(dags![hot_rank_pipeline])
//!         .worker(WorkerConfig {
//!             queues: vec!["default".into(), "ranking".into()],
//!             max_concurrent_workflows: 10,
//!             max_concurrent_activities: 25,
//!             ..WorkerConfig::default()
//!         })
//! }
//! ```
//!
//! # Features demonstrated
//!
//! - **`#[workflow]`** — durable, event-sourced workflow orchestration
//! - **`#[activity]`** — retriable tasks with timeouts and heartbeats
//! - **`#[dag]`** — scheduled DAG pipelines with topological dependencies
//! - **`RetryPolicy`** — fixed and exponential backoff strategies
//! - **`TriggerRule`** — DAG task dependency rules (AllSuccess, AllDone, etc.)
//! - **`WorkflowContext`** — replay-aware context with `is_replaying()` + `version()`
//! - **`ActivityContext`** — heartbeat support for long-running tasks
//! - **`HarvestBuilder`** — engine configuration with worker pools and queues
//! - **`workflows![]`**, **`activities![]`**, **`dags![]`** — collection macros
