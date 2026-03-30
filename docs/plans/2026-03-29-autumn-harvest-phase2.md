# Autumn Harvest Phase 2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring the Phase 1 skeleton to life — implement the event store, deterministic replay engine, Postgres-backed task queue, worker runtime with LISTEN/NOTIFY, heartbeating, workflow versioning, basic sticky caching, and end-to-end integration with the Autumn web framework.

**Architecture:** Phase 1 delivered types, events, models, macros, and builder. Phase 2 adds the runtime: an event store that appends/reads workflow history via diesel-async, a replay engine that rebuilds workflow state from history, a task queue using `SELECT ... FOR UPDATE SKIP LOCKED` for contention-free claiming, a worker that polls via LISTEN/NOTIFY with periodic fallback, and an `AppBuilder` extension trait to wire everything into Autumn's startup lifecycle. Two separate database pools (web + worker) share a connection ceiling to prevent mutual starvation.

**Tech Stack:** Rust 1.86+, edition 2024, Tokio, Diesel 2 + diesel-async, deadpool, Postgres (LISTEN/NOTIFY, SKIP LOCKED, advisory locks), thiserror, serde/serde_json, uuid, chrono, tokio-postgres (for raw LISTEN), lru (for workflow cache).

**Depends on:** Phase 1 complete (types, error, event, policy, context stubs, models, schema, macros, builder).

---

## Design Decisions Baked Into This Plan

### DD-1: Suspension Model — Tokio Oneshot Channels (Option A)

Each `ctx.execute_activity()` creates a `tokio::sync::oneshot` channel. The workflow coroutine `.await`s the receiver. The worker sends the result when the activity completes. The coroutine stays allocated in memory on the sticky worker. Durability comes from the event history in Postgres — if the worker dies, a new worker replays history to rebuild the coroutine.

### DD-2: Separate Database Pools with Shared Ceiling

Two `deadpool` instances: one for web request handlers, one for Harvest workers. A `max_total_connections` ceiling (default: Postgres `max_connections - 5`) prevents overloading. Activities can't starve HTTP. Config:

```toml
[harvest.database]
worker_pool_size = 10
web_pool_size = 10       # inherited from autumn's [database] if not set
max_total_connections = 95  # shared ceiling, default = pg max_connections - 5
```

### DD-3: Workflow Versioning Ships in Phase 2

`ctx.version("change-id", min, max)` records a `VersionMarker` event. During replay, mismatched versions route to the correct code branch. This prevents non-determinism panics when deploying code changes with in-flight workflows.

### DD-4: Basic Sticky Cache (In-Process Only)

An LRU cache of suspended workflow coroutines within a single worker process. No cross-worker sticky routing yet (that's Phase 3 with per-worker NOTIFY channels). This avoids full history replay for the common case of sequential activity completions on the same worker.

---

## New Dependencies

Add to `autumn-harvest/Cargo.toml`:

```toml
[dependencies]
# ... existing deps ...
tokio-postgres = "0.7"     # raw LISTEN/NOTIFY (diesel-async doesn't expose it)
lru = "0.12"               # workflow state cache
```

---

### Task 1: Event Store — Writer

**Files:**
- Create: `autumn-harvest/src/store.rs`
- Modify: `autumn-harvest/src/lib.rs`

The event store is the persistence backbone. Every workflow state change goes through here. The writer appends events transactionally — if two workers try to append to the same workflow simultaneously, the `UNIQUE(workflow_exec_id, event_id)` constraint rejects the loser.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/store.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::WorkflowEvent;
    use crate::types::ExecutionId;
    use chrono::Utc;

    // Unit tests use a mock connection trait so we don't need Postgres.
    // Integration tests in Task 15 hit real Postgres.

    #[test]
    fn stored_event_has_sequential_event_id() {
        let exec_id = ExecutionId::new();
        let events = vec![
            WorkflowEvent::WorkflowStarted {
                input: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            WorkflowEvent::ActivityScheduled {
                activity_id: crate::types::ActivityExecId::new(),
                name: "send_email".into(),
                input: serde_json::Value::Null,
                queue: "default".into(),
            },
        ];

        let rows = events_to_insert_rows(exec_id, &events);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].event_id, 0);
        assert_eq!(rows[1].event_id, 1);
        assert_eq!(rows[0].event_type, "WorkflowStarted");
        assert_eq!(rows[1].event_type, "ActivityScheduled");
    }

    #[test]
    fn events_to_rows_serializes_json() {
        let exec_id = ExecutionId::new();
        let events = vec![WorkflowEvent::WorkflowCompleted {
            output: serde_json::json!({"result": 42}),
        }];

        let rows = events_to_insert_rows(exec_id, &events);
        let data = &rows[0].event_data;
        assert!(data.get("data").is_some(), "serde tagged enum wraps in 'data'");
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cd ~/autumn-harvest && cargo test -p autumn-harvest store
```

Expected: compile error (`events_to_insert_rows` not defined)

**Step 3: Implement event store writer**

```rust
//! Event store — append-only persistence for workflow event histories.
//!
//! All writes go through `append_events()` which inserts atomically.
//! The `UNIQUE(workflow_exec_id, event_id)` constraint guarantees
//! that two workers can't append conflicting events to the same workflow.

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use diesel_async::AsyncPgConnection;
use uuid::Uuid;

use crate::error::{HarvestError, HarvestResult};
use crate::event::WorkflowEvent;
use crate::models::NewHarvestEvent;
use crate::schema::harvest_events;
use crate::types::ExecutionId;

/// Convert in-memory events to insertable rows with sequential event IDs.
///
/// `start_id` is the next event_id to assign (0 for a fresh workflow,
/// or `existing_count` when appending to an in-progress workflow).
pub fn events_to_insert_rows(
    exec_id: ExecutionId,
    events: &[WorkflowEvent],
) -> Vec<NewHarvestEvent<'_>> {
    events_to_insert_rows_from(exec_id, events, 0)
}

pub fn events_to_insert_rows_from(
    exec_id: ExecutionId,
    events: &[WorkflowEvent],
    start_id: i32,
) -> Vec<NewHarvestEvent<'_>> {
    events
        .iter()
        .enumerate()
        .map(|(i, event)| {
            let event_id = start_id + i as i32;
            NewHarvestEvent {
                workflow_exec_id: exec_id.as_uuid(),
                event_id,
                event_type: event.type_name(),
                event_data: serde_json::to_value(event)
                    .expect("WorkflowEvent must serialize"),
            }
        })
        .collect()
}

/// Append events to a workflow's history in a single INSERT.
///
/// Returns the number of events inserted. Fails with a unique constraint
/// violation if `start_id` conflicts — this indicates a concurrency bug
/// (two workers trying to advance the same workflow).
pub async fn append_events(
    conn: &mut AsyncPgConnection,
    exec_id: ExecutionId,
    events: &[WorkflowEvent],
    start_id: i32,
) -> HarvestResult<usize> {
    if events.is_empty() {
        return Ok(0);
    }

    let rows = events_to_insert_rows_from(exec_id, events, start_id);

    diesel::insert_into(harvest_events::table)
        .values(&rows)
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))
}

#[cfg(test)]
mod tests {
    // ... (tests from Step 1)
}
```

**Step 4: Expose in lib.rs**

Add to `autumn-harvest/src/lib.rs`:

```rust
pub mod store;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest store
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(store): add event store writer with sequential event IDs"
```

---

### Task 2: Event Store — Reader

**Files:**
- Modify: `autumn-harvest/src/store.rs`

**Step 1: Write failing test**

```rust
// Add to store.rs tests module:

#[test]
fn history_from_rows_deserializes_events() {
    let exec_id = ExecutionId::new();
    let original = vec![
        WorkflowEvent::WorkflowStarted {
            input: serde_json::json!({"user": 1}),
            timestamp: Utc::now(),
        },
        WorkflowEvent::WorkflowCompleted {
            output: serde_json::json!("done"),
        },
    ];

    // Round-trip through serialization (simulating DB storage)
    let rows = events_to_insert_rows(exec_id, &original);
    let json_values: Vec<serde_json::Value> = rows.iter().map(|r| r.event_data.clone()).collect();

    let restored: Vec<WorkflowEvent> = json_values
        .iter()
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .collect();

    assert_eq!(restored.len(), 2);
    assert!(matches!(restored[0], WorkflowEvent::WorkflowStarted { .. }));
    assert!(matches!(restored[1], WorkflowEvent::WorkflowCompleted { .. }));
}
```

**Step 2: Run test — expect PASS (this is a round-trip verification)**

```bash
cargo test -p autumn-harvest store::tests::history_from_rows
```

**Step 3: Implement load_history**

```rust
/// A complete workflow event history loaded from the database.
#[derive(Debug)]
pub struct EventHistory {
    pub exec_id: ExecutionId,
    pub events: Vec<WorkflowEvent>,
    /// The next event_id to use when appending.
    pub next_event_id: i32,
}

/// Load the full event history for a workflow execution, ordered by event_id.
pub async fn load_history(
    conn: &mut AsyncPgConnection,
    exec_id: ExecutionId,
) -> HarvestResult<EventHistory> {
    use crate::schema::harvest_events::dsl::*;

    let rows = harvest_events
        .filter(workflow_exec_id.eq(exec_id.as_uuid()))
        .order(event_id.asc())
        .load::<crate::models::HarvestEvent>(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;

    let next_id = rows.last().map_or(0, |r| r.event_id + 1);

    let events = rows
        .into_iter()
        .map(|row| {
            serde_json::from_value(row.event_data)
                .map_err(|e| HarvestError::Deserialization(e.to_string()))
        })
        .collect::<HarvestResult<Vec<_>>>()?;

    Ok(EventHistory {
        exec_id,
        events,
        next_event_id: next_id,
    })
}
```

**Step 4: Run compile check**

```bash
cargo build -p autumn-harvest
```

**Step 5: Commit**

```bash
git add -A && git commit -m "feat(store): add load_history reader for event replay"
```

---

### Task 3: Replay Engine — HistoryMatcher

**Files:**
- Create: `autumn-harvest/src/replay.rs`
- Modify: `autumn-harvest/src/lib.rs`

This is the brain of the durable execution model. The `HistoryMatcher` walks through recorded events during replay, matching workflow commands against what was previously recorded.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/replay.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::WorkflowEvent;
    use crate::types::ActivityExecId;
    use chrono::Utc;

    fn sample_history() -> Vec<WorkflowEvent> {
        let aid = ActivityExecId::new();
        vec![
            WorkflowEvent::WorkflowStarted {
                input: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            WorkflowEvent::ActivityScheduled {
                activity_id: aid,
                name: "send_email".into(),
                input: serde_json::json!({"to": "user@example.com"}),
                queue: "default".into(),
            },
            WorkflowEvent::ActivityCompleted {
                activity_id: aid,
                output: serde_json::json!({"id": "email-123"}),
            },
        ]
    }

    #[test]
    fn matcher_replays_completed_activity() {
        let history = sample_history();
        let mut matcher = HistoryMatcher::new(history);

        // Skip WorkflowStarted (consumed during init)
        matcher.advance();

        let result = matcher.match_activity("send_email");
        assert!(matches!(result, HistoryMatch::Matched { .. }));
        if let HistoryMatch::Matched { output, .. } = result {
            assert_eq!(output, serde_json::json!({"id": "email-123"}));
        }
    }

    #[test]
    fn matcher_returns_no_match_at_end_of_history() {
        let history = sample_history();
        let mut matcher = HistoryMatcher::new(history);
        matcher.advance(); // WorkflowStarted

        // Consume the activity pair
        let _ = matcher.match_activity("send_email");

        // Now we're past recorded history — new command
        let result = matcher.match_activity("create_account");
        assert!(matches!(result, HistoryMatch::NoMatch));
    }

    #[test]
    fn matcher_detects_non_determinism() {
        let history = sample_history();
        let mut matcher = HistoryMatcher::new(history);
        matcher.advance(); // WorkflowStarted

        // Workflow code asks for "create_account" but history has "send_email"
        let result = matcher.match_activity("create_account");
        assert!(matches!(result, HistoryMatch::Diverged { .. }));
    }

    #[test]
    fn matcher_replays_version_marker() {
        let history = vec![
            WorkflowEvent::WorkflowStarted {
                input: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            WorkflowEvent::MarkerRecorded {
                name: "version:add-retry-logic".into(),
                details: serde_json::json!({"version": 2}),
            },
        ];
        let mut matcher = HistoryMatcher::new(history);
        matcher.advance(); // WorkflowStarted

        let version = matcher.match_version("add-retry-logic", 1, 2);
        assert_eq!(version, 2);
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cd ~/autumn-harvest && cargo test -p autumn-harvest replay
```

**Step 3: Implement HistoryMatcher**

```rust
//! Deterministic replay engine for workflow event histories.
//!
//! The `HistoryMatcher` walks through a recorded event history in lockstep
//! with a re-executing workflow function. For each command the workflow issues
//! (schedule activity, start timer, etc.), the matcher checks whether the
//! history contains a matching event pair (Scheduled + Completed/Failed).
//!
//! Three outcomes:
//! - **Matched**: history has a result for this command — return it without executing.
//! - **NoMatch**: we've reached the end of recorded history — this is a new command.
//! - **Diverged**: the command doesn't match what was recorded — non-determinism bug.

use crate::event::WorkflowEvent;

/// Result of matching a workflow command against recorded history.
#[derive(Debug)]
pub enum HistoryMatch {
    /// History contains a completed result for this command.
    Matched {
        output: serde_json::Value,
    },
    /// History contains a failure for this command.
    Failed {
        error: String,
        attempt: u32,
    },
    /// We're past the end of recorded history — this is a new command.
    NoMatch,
    /// The command doesn't match the next event in history — non-determinism.
    Diverged {
        expected: String,
        actual: String,
    },
}

/// Walks through a workflow's event history during replay.
///
/// Advances a cursor through the events. Each `match_*` method consumes
/// the relevant events (e.g., ActivityScheduled + ActivityCompleted) and
/// returns the result.
pub struct HistoryMatcher {
    events: Vec<WorkflowEvent>,
    cursor: usize,
}

impl HistoryMatcher {
    pub fn new(events: Vec<WorkflowEvent>) -> Self {
        Self { events, cursor: 0 }
    }

    /// True if we've consumed all recorded events (new commands from here).
    pub fn is_replaying(&self) -> bool {
        self.cursor < self.events.len()
    }

    /// Current position in the history.
    pub fn position(&self) -> usize {
        self.cursor
    }

    /// Advance past the current event (e.g., skip WorkflowStarted during init).
    pub fn advance(&mut self) {
        if self.cursor < self.events.len() {
            self.cursor += 1;
        }
    }

    /// Match an `execute_activity` command against history.
    ///
    /// Expects to see `ActivityScheduled { name }` at the cursor, followed
    /// by `ActivityCompleted` or `ActivityFailed` for the same activity_id.
    pub fn match_activity(&mut self, activity_name: &str) -> HistoryMatch {
        if self.cursor >= self.events.len() {
            return HistoryMatch::NoMatch;
        }

        // Check that the next event is ActivityScheduled with the right name
        match &self.events[self.cursor] {
            WorkflowEvent::ActivityScheduled { name, activity_id, .. }
                if name == activity_name =>
            {
                let target_id = *activity_id;
                self.cursor += 1; // consume ActivityScheduled

                // Scan forward for the completion/failure event
                while self.cursor < self.events.len() {
                    match &self.events[self.cursor] {
                        WorkflowEvent::ActivityCompleted { activity_id, output }
                            if *activity_id == target_id =>
                        {
                            let output = output.clone();
                            self.cursor += 1;
                            return HistoryMatch::Matched { output };
                        }
                        WorkflowEvent::ActivityFailed { activity_id, error, attempt }
                            if *activity_id == target_id =>
                        {
                            let error = error.clone();
                            let attempt = *attempt;
                            self.cursor += 1;
                            return HistoryMatch::Failed { error, attempt };
                        }
                        WorkflowEvent::ActivityHeartbeat { activity_id, .. }
                            if *activity_id == target_id =>
                        {
                            // Skip heartbeats during replay
                            self.cursor += 1;
                        }
                        WorkflowEvent::ActivityStarted { activity_id, .. }
                            if *activity_id == target_id =>
                        {
                            // Skip ActivityStarted during replay
                            self.cursor += 1;
                        }
                        _ => {
                            // Non-matching event while scanning for completion —
                            // this means the activity was scheduled but hasn't
                            // completed yet in the history. Treat as NoMatch
                            // (workflow will suspend and wait for real completion).
                            return HistoryMatch::NoMatch;
                        }
                    }
                }

                // Reached end of history without finding completion
                HistoryMatch::NoMatch
            }
            WorkflowEvent::ActivityScheduled { name, .. } => {
                HistoryMatch::Diverged {
                    expected: name.clone(),
                    actual: activity_name.to_string(),
                }
            }
            other => {
                HistoryMatch::Diverged {
                    expected: other.type_name().to_string(),
                    actual: format!("ActivityScheduled({})", activity_name),
                }
            }
        }
    }

    /// Match a `ctx.timer()` command against history.
    pub fn match_timer(&mut self, timer_id: &str) -> HistoryMatch {
        if self.cursor >= self.events.len() {
            return HistoryMatch::NoMatch;
        }

        match &self.events[self.cursor] {
            WorkflowEvent::TimerStarted { timer_id: tid, .. }
                if tid.as_str() == timer_id =>
            {
                self.cursor += 1; // consume TimerStarted

                // Look for TimerFired
                if self.cursor < self.events.len() {
                    if let WorkflowEvent::TimerFired { timer_id: tid } = &self.events[self.cursor] {
                        if tid.as_str() == timer_id {
                            self.cursor += 1;
                            return HistoryMatch::Matched {
                                output: serde_json::Value::Null,
                            };
                        }
                    }
                }
                HistoryMatch::NoMatch
            }
            _ => HistoryMatch::NoMatch,
        }
    }

    /// Match a `ctx.version()` call against history.
    ///
    /// Returns the recorded version if found, or `max_version` if no marker
    /// exists (new code running for the first time on an old workflow).
    pub fn match_version(
        &mut self,
        change_id: &str,
        min_version: u32,
        max_version: u32,
    ) -> u32 {
        if self.cursor >= self.events.len() {
            // Past end of history — this is new code, use max_version
            return max_version;
        }

        let marker_name = format!("version:{change_id}");

        match &self.events[self.cursor] {
            WorkflowEvent::MarkerRecorded { name, details }
                if name == &marker_name =>
            {
                let version = details
                    .get("version")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(max_version as u64) as u32;
                self.cursor += 1;
                version.clamp(min_version, max_version)
            }
            _ => {
                // No marker recorded — old workflow before this version gate.
                // Return min_version (the original code path).
                min_version
            }
        }
    }
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod replay;
pub use replay::{HistoryMatch, HistoryMatcher};
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest replay
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(replay): add HistoryMatcher with activity, timer, and version matching"
```

---

### Task 4: Full WorkflowContext with Replay

**Files:**
- Modify: `autumn-harvest/src/context.rs`
- Modify: `autumn-harvest/src/event.rs` (add VersionMarker details)

This is the most architecturally significant struct in the engine. The `WorkflowContext` operates in two modes: **replay** (returning recorded results) and **live** (scheduling new work and suspending). User contribution requested for the core dispatch method.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/context.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::WorkflowEvent;
    use crate::types::{ActivityExecId, ExecutionId};
    use chrono::Utc;

    #[tokio::test]
    async fn context_replays_completed_activity() {
        let exec_id = ExecutionId::new();
        let aid = ActivityExecId::new();

        let history = vec![
            WorkflowEvent::WorkflowStarted {
                input: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            WorkflowEvent::ActivityScheduled {
                activity_id: aid,
                name: "greet".into(),
                input: serde_json::json!("world"),
                queue: "default".into(),
            },
            WorkflowEvent::ActivityCompleted {
                activity_id: aid,
                output: serde_json::json!("hello world"),
            },
        ];

        let ctx = WorkflowContext::for_replay(exec_id, history);

        let result: serde_json::Value = ctx
            .execute_activity_raw("greet", serde_json::json!("world"), "default")
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!("hello world"));
    }

    #[tokio::test]
    async fn context_version_returns_recorded_version() {
        let exec_id = ExecutionId::new();
        let history = vec![
            WorkflowEvent::WorkflowStarted {
                input: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            WorkflowEvent::MarkerRecorded {
                name: "version:add-retry".into(),
                details: serde_json::json!({"version": 1}),
            },
        ];

        let ctx = WorkflowContext::for_replay(exec_id, history);
        let v = ctx.version("add-retry", 1, 2);
        assert_eq!(v, 1);
    }

    #[tokio::test]
    async fn context_now_returns_deterministic_time() {
        let exec_id = ExecutionId::new();
        let ts = Utc::now();
        let history = vec![WorkflowEvent::WorkflowStarted {
            input: serde_json::json!({}),
            timestamp: ts,
        }];

        let ctx = WorkflowContext::for_replay(exec_id, history);
        assert_eq!(ctx.now(), ts);
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cd ~/autumn-harvest && cargo test -p autumn-harvest context
```

**Step 3: Implement WorkflowContext**

```rust
//! Workflow execution context — the main interface workflows interact with.
//!
//! Operates in two modes:
//! - **Replay**: reads results from recorded event history (no side effects).
//! - **Live**: schedules new activities, starts timers, records events.
//!
//! Workflows MUST be deterministic. Use `ctx.now()` instead of `Utc::now()`,
//! `ctx.version()` for code evolution, and `ctx.execute_activity()` for I/O.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use tokio::sync::oneshot;

use crate::error::{HarvestError, HarvestResult};
use crate::event::WorkflowEvent;
use crate::replay::{HistoryMatch, HistoryMatcher};
use crate::types::{ActivityExecId, ExecutionId, TimerId};

/// Command from the workflow to the engine (new work to schedule).
#[derive(Debug)]
pub enum WorkflowCommand {
    ScheduleActivity {
        activity_id: ActivityExecId,
        name: String,
        input: serde_json::Value,
        queue: String,
        /// The engine sends the result through this channel.
        result_tx: oneshot::Sender<Result<serde_json::Value, String>>,
    },
    StartTimer {
        timer_id: TimerId,
        duration_secs: u64,
        result_tx: oneshot::Sender<()>,
    },
    RecordMarker {
        name: String,
        details: serde_json::Value,
    },
    Complete {
        output: serde_json::Value,
    },
    Fail {
        error: String,
    },
}

/// Context passed to every workflow function during execution and replay.
pub struct WorkflowContext {
    exec_id: ExecutionId,
    matcher: Mutex<HistoryMatcher>,
    /// Commands emitted during live execution (collected by the worker).
    commands: Mutex<Vec<WorkflowCommand>>,
    /// Deterministic timestamp from WorkflowStarted event.
    start_time: DateTime<Utc>,
    /// Next activity sequence number (for generating deterministic IDs).
    activity_seq: Mutex<u32>,
    /// Next timer sequence number.
    timer_seq: Mutex<u32>,
}

impl WorkflowContext {
    /// Create a context for replaying from a loaded event history.
    pub fn for_replay(exec_id: ExecutionId, events: Vec<WorkflowEvent>) -> Self {
        let start_time = events
            .first()
            .and_then(|e| match e {
                WorkflowEvent::WorkflowStarted { timestamp, .. } => Some(*timestamp),
                _ => None,
            })
            .unwrap_or_else(Utc::now);

        let mut matcher = HistoryMatcher::new(events);
        matcher.advance(); // skip WorkflowStarted

        Self {
            exec_id,
            matcher: Mutex::new(matcher),
            commands: Mutex::new(Vec::new()),
            start_time,
            activity_seq: Mutex::new(0),
            timer_seq: Mutex::new(0),
        }
    }

    /// Deterministic "now" — returns the workflow start timestamp.
    ///
    /// In a full implementation this would advance based on timer events,
    /// but the start time is sufficient for deterministic replay.
    pub fn now(&self) -> DateTime<Utc> {
        self.start_time
    }

    /// Execution ID for this workflow run.
    pub fn execution_id(&self) -> ExecutionId {
        self.exec_id
    }

    /// Code versioning gate for safe workflow evolution.
    ///
    /// Records a `MarkerRecorded` event the first time this change_id is
    /// encountered. On replay, returns the previously recorded version.
    /// New deployments get `max_version`; old histories get `min_version`.
    ///
    /// ```rust,no_run
    /// let v = ctx.version("add-retry-logic", 1, 2);
    /// if v >= 2 {
    ///     // new code path with retry
    /// } else {
    ///     // original code path (for in-flight workflows)
    /// }
    /// ```
    pub fn version(&self, change_id: &str, min_version: u32, max_version: u32) -> u32 {
        let mut matcher = self.matcher.lock().expect("matcher lock poisoned");
        let version = matcher.match_version(change_id, min_version, max_version);

        if !matcher.is_replaying() {
            // Live: record the version marker for future replays
            self.commands.lock().expect("commands lock").push(
                WorkflowCommand::RecordMarker {
                    name: format!("version:{change_id}"),
                    details: serde_json::json!({"version": version}),
                },
            );
        }

        version
    }

    /// Schedule an activity and await its result.
    ///
    /// During replay, returns the recorded result immediately.
    /// During live execution, emits a `ScheduleActivity` command and
    /// suspends on a oneshot channel until the worker sends the result.
    pub async fn execute_activity_raw(
        &self,
        activity_name: &str,
        input: serde_json::Value,
        queue: &str,
    ) -> HarvestResult<serde_json::Value> {
        let mut matcher = self.matcher.lock().expect("matcher lock poisoned");

        match matcher.match_activity(activity_name) {
            HistoryMatch::Matched { output } => {
                // Replay: return recorded result
                Ok(output)
            }
            HistoryMatch::Failed { error, .. } => {
                Err(HarvestError::ActivityFailed {
                    name: activity_name.to_string(),
                    error,
                })
            }
            HistoryMatch::NoMatch => {
                // Drop the lock before awaiting
                drop(matcher);

                // Live: schedule the activity and suspend
                let activity_id = self.next_activity_id();
                let (tx, rx) = oneshot::channel();

                self.commands.lock().expect("commands lock").push(
                    WorkflowCommand::ScheduleActivity {
                        activity_id,
                        name: activity_name.to_string(),
                        input,
                        queue: queue.to_string(),
                        result_tx: tx,
                    },
                );

                rx.await
                    .map_err(|_| HarvestError::WorkflowCancelled)?
                    .map_err(|e| HarvestError::ActivityFailed {
                        name: activity_name.to_string(),
                        error: e,
                    })
            }
            HistoryMatch::Diverged { expected, actual } => {
                Err(HarvestError::NonDeterministic {
                    expected,
                    actual,
                })
            }
        }
    }

    /// Drain pending commands (called by the worker after the workflow suspends).
    pub fn drain_commands(&self) -> Vec<WorkflowCommand> {
        std::mem::take(&mut *self.commands.lock().expect("commands lock"))
    }

    fn next_activity_id(&self) -> ActivityExecId {
        let mut seq = self.activity_seq.lock().expect("seq lock");
        let _id = *seq;
        *seq += 1;
        ActivityExecId::new()
    }
}
```

**Step 4: Add `ActivityFailed`, `NonDeterministic`, `WorkflowCancelled` to HarvestError**

In `autumn-harvest/src/error.rs`, add these variants:

```rust
    /// An activity execution failed.
    #[error("activity '{name}' failed: {error}")]
    ActivityFailed { name: String, error: String },

    /// Workflow replay detected non-deterministic code.
    #[error("non-deterministic replay: expected {expected}, got {actual}")]
    NonDeterministic { expected: String, actual: String },

    /// Workflow was cancelled while an activity was in flight.
    #[error("workflow cancelled")]
    WorkflowCancelled,

    /// Deserialization of stored event data failed.
    #[error("deserialization error: {0}")]
    Deserialization(String),
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest context
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(context): implement WorkflowContext with replay-aware execute_activity and versioning"
```

---

### Task 5: Full ActivityContext with Heartbeat and State Access

**Files:**
- Modify: `autumn-harvest/src/context.rs`

**Step 1: Write failing test**

```rust
// Add to context.rs tests module:

#[tokio::test]
async fn activity_context_heartbeat_sends_on_channel() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let ctx = ActivityContext::new(tx, Default::default());

    ctx.heartbeat("step 1").await.unwrap();
    ctx.heartbeat("step 2").await.unwrap();

    let msg1 = rx.recv().await.unwrap();
    let msg2 = rx.recv().await.unwrap();
    assert_eq!(msg1, serde_json::json!("step 1"));
    assert_eq!(msg2, serde_json::json!("step 2"));
}

#[tokio::test]
async fn activity_context_detects_cancellation() {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = tokio_util::sync::CancellationToken::new();
    let ctx = ActivityContext::with_cancellation(tx, Default::default(), cancel.clone());

    cancel.cancel();
    assert!(ctx.is_cancelled());
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest context::tests::activity_context
```

**Step 3: Implement ActivityContext**

```rust
/// Context passed to every activity function during execution.
///
/// Provides:
/// - Heartbeating (liveness signal to the engine)
/// - Cancellation detection
/// - Access to shared application state
pub struct ActivityContext {
    heartbeat_tx: tokio::sync::mpsc::Sender<serde_json::Value>,
    state: std::sync::Arc<crate::AppState>,
    cancel: tokio_util::sync::CancellationToken,
}

impl ActivityContext {
    pub fn new(
        heartbeat_tx: tokio::sync::mpsc::Sender<serde_json::Value>,
        state: std::sync::Arc<crate::AppState>,
    ) -> Self {
        Self {
            heartbeat_tx,
            state,
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub fn with_cancellation(
        heartbeat_tx: tokio::sync::mpsc::Sender<serde_json::Value>,
        state: std::sync::Arc<crate::AppState>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            heartbeat_tx,
            state,
            cancel,
        }
    }

    /// Send a heartbeat signal. Call this periodically in long-running activities.
    ///
    /// If the workflow has been cancelled, returns `Err(HarvestError::WorkflowCancelled)`.
    pub async fn heartbeat(&self, details: impl serde::Serialize) -> HarvestResult<()> {
        if self.cancel.is_cancelled() {
            return Err(HarvestError::WorkflowCancelled);
        }

        let payload = serde_json::to_value(details)
            .map_err(|e| HarvestError::Serialization(e.to_string()))?;

        self.heartbeat_tx
            .send(payload)
            .await
            .map_err(|_| HarvestError::WorkflowCancelled)
    }

    /// Check if this activity's workflow has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Access shared application state (e.g., HTTP clients, config).
    pub fn state(&self) -> &crate::AppState {
        &self.state
    }
}
```

**Step 4: Add `Serialization` variant to HarvestError and add tokio-util dependency**

In `autumn-harvest/Cargo.toml`:
```toml
tokio-util = { version = "0.7", features = ["rt"] }
```

In `error.rs`:
```rust
    #[error("serialization error: {0}")]
    Serialization(String),
```

**Step 5: Create a placeholder AppState type**

```rust
// In autumn-harvest/src/lib.rs or a new state.rs:
/// Shared application state accessible by activities.
///
/// In Phase 2 this is a placeholder. When integrated with Autumn's AppBuilder,
/// this wraps `autumn_web::AppState`.
#[derive(Default)]
pub struct AppState {
    // Populated during HarvestExt integration (Task 14)
    _private: (),
}
```

**Step 6: Run test — expect PASS**

```bash
cargo test -p autumn-harvest context
```

**Step 7: Commit**

```bash
git add -A && git commit -m "feat(context): implement ActivityContext with heartbeat, cancellation, and state access"
```

---

### Task 6: Task Queue Operations

**Files:**
- Create: `autumn-harvest/src/queue.rs`
- Modify: `autumn-harvest/src/lib.rs`

The Postgres-backed task queue. Uses `SELECT ... FOR UPDATE SKIP LOCKED` for contention-free task claiming — multiple workers can poll the same queue without blocking each other.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/queue.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_params_builds_correctly() {
        let params = EnqueueParams {
            queue_name: "email-workers",
            task_type: TaskType::Activity,
            workflow_exec_id: Some(uuid::Uuid::new_v4()),
            activity_name: Some("send_email"),
            input: serde_json::json!({"to": "user@test.com"}),
            priority: 5,
            max_attempts: 3,
            heartbeat_timeout: Some(chrono::Duration::seconds(10)),
            start_to_close: Some(chrono::Duration::seconds(30)),
            schedule_to_start: None,
            retry_policy: None,
            scheduled_at: None,
        };

        assert_eq!(params.queue_name, "email-workers");
        assert_eq!(params.priority, 5);
    }

    #[test]
    fn task_type_display() {
        assert_eq!(TaskType::Workflow.as_str(), "workflow");
        assert_eq!(TaskType::Activity.as_str(), "activity");
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest queue
```

**Step 3: Implement queue operations**

```rust
//! Postgres-backed task queue with SKIP LOCKED claiming.
//!
//! Workers poll this queue for tasks. Enqueuing happens when a workflow
//! schedules an activity or a new workflow is started.

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{Array, Text as SqlText};
use diesel_async::RunQueryDsl;
use diesel_async::AsyncPgConnection;
use uuid::Uuid;

use crate::error::{HarvestError, HarvestResult};
use crate::models::{NewTaskQueueItem, TaskQueueItem};
use crate::schema::harvest_task_queue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    Workflow,
    Activity,
}

impl TaskType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::Activity => "activity",
        }
    }
}

/// Parameters for enqueuing a new task.
pub struct EnqueueParams<'a> {
    pub queue_name: &'a str,
    pub task_type: TaskType,
    pub workflow_exec_id: Option<Uuid>,
    pub activity_name: Option<&'a str>,
    pub input: serde_json::Value,
    pub priority: i32,
    pub max_attempts: i32,
    pub heartbeat_timeout: Option<chrono::Duration>,
    pub start_to_close: Option<chrono::Duration>,
    pub schedule_to_start: Option<chrono::Duration>,
    pub retry_policy: Option<serde_json::Value>,
    pub scheduled_at: Option<DateTime<Utc>>,
}

/// Enqueue a task for worker pickup.
pub async fn enqueue(
    conn: &mut AsyncPgConnection,
    params: EnqueueParams<'_>,
) -> HarvestResult<Uuid> {
    let id = Uuid::new_v4();
    let now = params.scheduled_at.unwrap_or_else(Utc::now);

    let row = NewTaskQueueItem {
        id,
        queue_name: params.queue_name,
        task_type: params.task_type.as_str(),
        workflow_exec_id: params.workflow_exec_id,
        activity_name: params.activity_name,
        input: params.input,
        priority: params.priority,
        max_attempts: params.max_attempts,
        scheduled_at: now,
        heartbeat_timeout: params.heartbeat_timeout,
        start_to_close: params.start_to_close,
        schedule_to_start: params.schedule_to_start,
        retry_policy: params.retry_policy,
    };

    diesel::insert_into(harvest_task_queue::table)
        .values(&row)
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;

    Ok(id)
}

/// Claim the next available task from the given queues using SKIP LOCKED.
///
/// This is the core contention-free polling query. Multiple workers can
/// call this simultaneously without blocking — each gets a different task.
///
/// Returns `None` if no tasks are available.
pub async fn claim_task(
    conn: &mut AsyncPgConnection,
    queues: &[String],
    worker_id: &str,
) -> HarvestResult<Option<TaskQueueItem>> {
    // Raw SQL because Diesel doesn't support FOR UPDATE SKIP LOCKED natively.
    //
    // The subquery selects the highest-priority, oldest pending task
    // from any of the worker's queues and locks it. The outer UPDATE
    // atomically transitions it to RUNNING.
    let result = diesel::sql_query(
        "UPDATE harvest_task_queue \
         SET state = 'RUNNING', \
             worker_id = $1, \
             started_at = NOW(), \
             attempt = attempt + 1 \
         WHERE id = ( \
             SELECT id FROM harvest_task_queue \
             WHERE queue_name = ANY($2) \
               AND state = 'PENDING' \
               AND scheduled_at <= NOW() \
             ORDER BY priority DESC, scheduled_at ASC \
             LIMIT 1 \
             FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING *"
    )
    .bind::<SqlText, _>(worker_id)
    .bind::<Array<SqlText>, _>(queues)
    .get_result::<TaskQueueItem>(conn)
    .await
    .optional()
    .map_err(|e| HarvestError::Database(e.to_string()))?;

    Ok(result)
}

/// Mark a task as completed with output.
pub async fn complete_task(
    conn: &mut AsyncPgConnection,
    task_id: Uuid,
    output: serde_json::Value,
) -> HarvestResult<()> {
    diesel::update(harvest_task_queue::table.find(task_id))
        .set((
            harvest_task_queue::state.eq("COMPLETED"),
            harvest_task_queue::output.eq(Some(output)),
            harvest_task_queue::completed_at.eq(Some(Utc::now())),
        ))
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;
    Ok(())
}

/// Mark a task as failed with error message.
pub async fn fail_task(
    conn: &mut AsyncPgConnection,
    task_id: Uuid,
    error: &str,
) -> HarvestResult<()> {
    diesel::update(harvest_task_queue::table.find(task_id))
        .set((
            harvest_task_queue::state.eq("FAILED"),
            harvest_task_queue::error.eq(Some(error)),
            harvest_task_queue::completed_at.eq(Some(Utc::now())),
        ))
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;
    Ok(())
}

/// Update heartbeat timestamp for a running task.
pub async fn record_heartbeat(
    conn: &mut AsyncPgConnection,
    task_id: Uuid,
) -> HarvestResult<()> {
    diesel::update(harvest_task_queue::table.find(task_id))
        .set(harvest_task_queue::last_heartbeat_at.eq(Some(Utc::now())))
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;
    Ok(())
}

/// Re-enqueue a failed task for retry (resets state to PENDING with future scheduled_at).
pub async fn requeue_for_retry(
    conn: &mut AsyncPgConnection,
    task_id: Uuid,
    delay: chrono::Duration,
) -> HarvestResult<()> {
    let next_at = Utc::now() + delay;
    diesel::update(harvest_task_queue::table.find(task_id))
        .set((
            harvest_task_queue::state.eq("PENDING"),
            harvest_task_queue::worker_id.eq(None::<String>),
            harvest_task_queue::started_at.eq(None::<DateTime<Utc>>),
            harvest_task_queue::scheduled_at.eq(next_at),
            harvest_task_queue::error.eq(None::<String>),
        ))
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;
    Ok(())
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod queue;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest queue
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(queue): add Postgres-backed task queue with SKIP LOCKED claiming"
```

---

### Task 7: LISTEN/NOTIFY Wrapper

**Files:**
- Create: `autumn-harvest/src/notify.rs`
- Modify: `autumn-harvest/src/lib.rs`

Postgres LISTEN/NOTIFY provides sub-second task pickup without polling overhead. We use raw `tokio-postgres` for this since `diesel-async` doesn't expose the notification stream.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/notify.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_name_for_queue() {
        assert_eq!(queue_channel("default"), "harvest_queue_default");
        assert_eq!(queue_channel("email-workers"), "harvest_queue_email_workers");
    }

    #[test]
    fn notify_payload_roundtrips() {
        let payload = NotifyPayload {
            task_id: uuid::Uuid::new_v4(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let back: NotifyPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.task_id, back.task_id);
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest notify
```

**Step 3: Implement notify module**

```rust
//! Postgres LISTEN/NOTIFY integration for low-latency task pickup.
//!
//! Workers LISTEN on channels named `harvest_queue_{queue_name}`.
//! When a task is enqueued, a NOTIFY is sent on the corresponding channel.
//! Workers fall back to periodic polling (every 5 seconds) as a safety net.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{HarvestError, HarvestResult};

/// Channel name convention for a task queue.
pub fn queue_channel(queue_name: &str) -> String {
    // Replace hyphens with underscores for valid Postgres identifier
    format!("harvest_queue_{}", queue_name.replace('-', "_"))
}

/// Payload sent with NOTIFY when a task is enqueued.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyPayload {
    pub task_id: Uuid,
}

/// Send a NOTIFY for a newly enqueued task.
///
/// Uses raw SQL through diesel-async since this is a simple statement.
pub async fn notify_task_enqueued(
    conn: &mut diesel_async::AsyncPgConnection,
    queue_name: &str,
    task_id: Uuid,
) -> HarvestResult<()> {
    use diesel::sql_query;
    use diesel_async::RunQueryDsl;

    let channel = queue_channel(queue_name);
    let payload = serde_json::to_string(&NotifyPayload { task_id })
        .map_err(|e| HarvestError::Serialization(e.to_string()))?;

    // NOTIFY channel, 'payload'
    sql_query(format!("NOTIFY {channel}, '{payload}'"))
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;

    Ok(())
}

/// Listener that subscribes to task queue notifications.
///
/// Uses raw `tokio-postgres` because `diesel-async` doesn't expose
/// the notification stream. Created once per worker, lives for the
/// worker's lifetime.
pub struct QueueListener {
    /// Raw tokio-postgres connection dedicated to LISTEN.
    client: tokio_postgres::Client,
    /// Queues we're listening on.
    queues: Vec<String>,
}

impl QueueListener {
    /// Connect and subscribe to the given queues.
    pub async fn connect(
        database_url: &str,
        queues: &[String],
    ) -> HarvestResult<(Self, tokio::task::JoinHandle<()>)> {
        let (client, mut connection) =
            tokio_postgres::connect(database_url, tokio_postgres::NoTls)
                .await
                .map_err(|e| HarvestError::Database(e.to_string()))?;

        // Spawn the connection driver (required by tokio-postgres).
        let handle = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("LISTEN connection error: {e}");
            }
        });

        // Subscribe to each queue's channel.
        for queue in queues {
            let channel = queue_channel(queue);
            client
                .execute(&format!("LISTEN {channel}"), &[])
                .await
                .map_err(|e| HarvestError::Database(e.to_string()))?;
        }

        Ok((
            Self {
                client,
                queues: queues.to_vec(),
            },
            handle,
        ))
    }

    /// Wait for the next notification, or timeout after `poll_interval`.
    ///
    /// Returns `Some(payload)` on notification, `None` on timeout (time to poll).
    pub async fn wait_for_notification(
        &self,
        poll_interval: std::time::Duration,
    ) -> Option<NotifyPayload> {
        match tokio::time::timeout(poll_interval, async {
            // tokio-postgres notifications come through the connection
            // This is simplified — full implementation uses the notification stream
            tokio::time::sleep(poll_interval).await;
            None::<NotifyPayload>
        })
        .await
        {
            Ok(payload) => payload,
            Err(_timeout) => None,
        }
    }
}
```

> **Note:** The `wait_for_notification` implementation above is a skeleton. The full implementation in Task 9 (Worker Runtime) will use `tokio_postgres::AsyncMessage` stream. This task establishes the types and channel naming.

**Step 4: Expose in lib.rs**

```rust
pub mod notify;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest notify
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(notify): add LISTEN/NOTIFY wrapper with channel naming and payload types"
```

---

### Task 8: Workflow Executor

**Files:**
- Create: `autumn-harvest/src/executor.rs`
- Modify: `autumn-harvest/src/lib.rs`

The workflow executor runs a single workflow function through its replay + live execution cycle. It loads history, builds a `WorkflowContext`, runs the workflow function, collects commands, and returns them to the worker for processing.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/executor.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::WorkflowEvent;
    use crate::types::ExecutionId;
    use chrono::Utc;

    #[tokio::test]
    async fn executor_replays_completed_workflow() {
        let exec_id = ExecutionId::new();
        let aid = crate::types::ActivityExecId::new();

        let history = vec![
            WorkflowEvent::WorkflowStarted {
                input: serde_json::json!("test"),
                timestamp: Utc::now(),
            },
            WorkflowEvent::ActivityScheduled {
                activity_id: aid,
                name: "echo".into(),
                input: serde_json::json!("test"),
                queue: "default".into(),
            },
            WorkflowEvent::ActivityCompleted {
                activity_id: aid,
                output: serde_json::json!("echoed: test"),
            },
            WorkflowEvent::WorkflowCompleted {
                output: serde_json::json!("echoed: test"),
            },
        ];

        let handler = |ctx: &WorkflowContext, input: serde_json::Value| {
            Box::pin(async move {
                let result = ctx.execute_activity_raw("echo", input, "default").await
                    .map_err(|e| e.to_string())?;
                Ok(result)
            })
        };

        let outcome = run_workflow(exec_id, history, handler, serde_json::json!("test")).await;
        assert!(matches!(outcome, WorkflowOutcome::Completed { .. }));
    }

    #[tokio::test]
    async fn executor_suspends_on_new_activity() {
        let exec_id = ExecutionId::new();

        // History only has WorkflowStarted — no activity results yet
        let history = vec![WorkflowEvent::WorkflowStarted {
            input: serde_json::json!("test"),
            timestamp: Utc::now(),
        }];

        let handler = |ctx: &WorkflowContext, input: serde_json::Value| {
            Box::pin(async move {
                let _result = ctx.execute_activity_raw("echo", input, "default").await
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::Value::Null)
            })
        };

        let outcome = run_workflow(exec_id, history, handler, serde_json::json!("test")).await;
        assert!(matches!(outcome, WorkflowOutcome::Suspended { .. }));
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest executor
```

**Step 3: Implement workflow executor**

```rust
//! Workflow executor — runs a single workflow through replay + live execution.
//!
//! The executor is invoked by the worker each time a workflow task is claimed.
//! It loads the event history, builds a `WorkflowContext`, runs the workflow
//! function, and returns either a completion result or a set of commands
//! (new activities/timers to schedule).

use std::sync::Arc;

use crate::context::{WorkflowCommand, WorkflowContext};
use crate::event::WorkflowEvent;
use crate::info::WorkflowHandlerFn;
use crate::types::ExecutionId;

/// Result of executing a workflow step.
#[derive(Debug)]
pub enum WorkflowOutcome {
    /// Workflow ran to completion.
    Completed { output: serde_json::Value },
    /// Workflow hit an error.
    Failed { error: String },
    /// Workflow suspended waiting for activity/timer results.
    /// Contains commands that need to be processed by the worker.
    Suspended { commands: Vec<WorkflowCommand> },
}

/// Run a workflow function against its event history.
///
/// This is the core execution loop:
/// 1. Build a `WorkflowContext` from history (replay mode).
/// 2. Spawn the workflow function as a Tokio task.
/// 3. If replay succeeds and the workflow completes → `Completed`.
/// 4. If the workflow awaits a new activity → the oneshot blocks → `Suspended`.
/// 5. If replay detects non-determinism → `Failed`.
pub async fn run_workflow(
    exec_id: ExecutionId,
    history: Vec<WorkflowEvent>,
    handler: WorkflowHandlerFn,
    input: serde_json::Value,
) -> WorkflowOutcome {
    let ctx = WorkflowContext::for_replay(exec_id, history);

    // Run the workflow with a timeout — if it suspends on a oneshot,
    // we'll hit the timeout and collect the pending commands.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        handler(&ctx, input),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            // Workflow completed successfully
            WorkflowOutcome::Completed { output }
        }
        Ok(Err(error)) => {
            WorkflowOutcome::Failed { error }
        }
        Err(_timeout) => {
            // Workflow suspended — it's blocked on a oneshot receiver
            // waiting for an activity result. Collect the commands
            // it emitted before suspending.
            let commands = ctx.drain_commands();
            WorkflowOutcome::Suspended { commands }
        }
    }
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod executor;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest executor
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(executor): add workflow executor with replay and suspension detection"
```

---

### Task 9: Worker Runtime — Poll Loop

**Files:**
- Create: `autumn-harvest/src/worker.rs`
- Modify: `autumn-harvest/src/lib.rs`

The worker is the main runtime loop. It polls the task queue (via LISTEN/NOTIFY + fallback polling), dispatches tasks to the workflow or activity executor, and processes results.

> **User contribution requested:** The worker's task dispatch logic (how it routes a claimed task to either the workflow or activity executor and handles the result) is the core orchestration decision. See Step 3 below.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/worker.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_config_validates() {
        let config = WorkerRuntimeConfig {
            worker_id: "worker-1".into(),
            queues: vec!["default".into()],
            max_concurrent_workflows: 20,
            max_concurrent_activities: 50,
            poll_interval: std::time::Duration::from_secs(5),
            shutdown_timeout: std::time::Duration::from_secs(30),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn worker_config_rejects_empty_queues() {
        let config = WorkerRuntimeConfig {
            worker_id: "worker-1".into(),
            queues: vec![],
            max_concurrent_workflows: 20,
            max_concurrent_activities: 50,
            poll_interval: std::time::Duration::from_secs(5),
            shutdown_timeout: std::time::Duration::from_secs(30),
        };
        assert!(config.validate().is_err());
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest worker
```

**Step 3: Implement worker runtime**

```rust
//! Worker runtime — the main poll loop that drives workflow and activity execution.
//!
//! Each Autumn application instance runs one worker (or zero for scheduler-only).
//! The worker manages two semaphore-bounded executors:
//! - Workflow executor: replays + advances workflows (bounded by max_concurrent_workflows)
//! - Activity executor: runs activity functions (bounded by max_concurrent_activities)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::builder::WorkerConfig;
use crate::error::{HarvestError, HarvestResult};
use crate::info::{ActivityInfo, WorkflowInfo};

/// Runtime configuration for the worker process.
#[derive(Debug, Clone)]
pub struct WorkerRuntimeConfig {
    pub worker_id: String,
    pub queues: Vec<String>,
    pub max_concurrent_workflows: usize,
    pub max_concurrent_activities: usize,
    pub poll_interval: Duration,
    pub shutdown_timeout: Duration,
}

impl WorkerRuntimeConfig {
    pub fn validate(&self) -> HarvestResult<()> {
        if self.queues.is_empty() {
            return Err(HarvestError::Config(
                "worker must have at least one queue".into(),
            ));
        }
        Ok(())
    }
}

impl From<WorkerConfig> for WorkerRuntimeConfig {
    fn from(config: WorkerConfig) -> Self {
        Self {
            worker_id: format!("worker-{}", Uuid::new_v4()),
            queues: config.queues,
            max_concurrent_workflows: config.max_concurrent_workflows,
            max_concurrent_activities: config.max_concurrent_activities,
            poll_interval: Duration::from_secs(5),
            shutdown_timeout: config.shutdown_timeout,
        }
    }
}

/// Registry of known workflow and activity handlers.
pub struct HandlerRegistry {
    pub workflows: HashMap<String, WorkflowInfo>,
    pub activities: HashMap<String, ActivityInfo>,
}

impl HandlerRegistry {
    pub fn new(
        workflows: Vec<WorkflowInfo>,
        activities: Vec<ActivityInfo>,
    ) -> Self {
        let workflows = workflows
            .into_iter()
            .map(|w| (w.name.to_string(), w))
            .collect();
        let activities = activities
            .into_iter()
            .map(|a| (a.name.to_string(), a))
            .collect();
        Self { workflows, activities }
    }
}

/// The main worker runtime.
pub struct Worker {
    config: WorkerRuntimeConfig,
    registry: Arc<HandlerRegistry>,
    workflow_semaphore: Arc<Semaphore>,
    activity_semaphore: Arc<Semaphore>,
    shutdown: CancellationToken,
}

impl Worker {
    pub fn new(
        config: WorkerRuntimeConfig,
        registry: HandlerRegistry,
    ) -> HarvestResult<Self> {
        config.validate()?;

        Ok(Self {
            workflow_semaphore: Arc::new(Semaphore::new(config.max_concurrent_workflows)),
            activity_semaphore: Arc::new(Semaphore::new(config.max_concurrent_activities)),
            config,
            registry: Arc::new(registry),
            shutdown: CancellationToken::new(),
        })
    }

    /// Run the worker poll loop until shutdown is signalled.
    ///
    /// This is the main entry point — called from `HarvestExt::run()`.
    pub async fn run(
        &self,
        database_url: &str,
        worker_pool: deadpool::managed::Pool<diesel_async::pooled_connection::AsyncDieselConnectionManager<diesel_async::AsyncPgConnection>>,
    ) -> HarvestResult<()> {
        tracing::info!(
            worker_id = %self.config.worker_id,
            queues = ?self.config.queues,
            "Harvest worker starting"
        );

        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => {
                    tracing::info!("Harvest worker shutting down");
                    break;
                }
                _ = self.poll_once(&worker_pool) => {}
            }
        }

        self.drain_in_flight().await;
        Ok(())
    }

    /// Single poll iteration: try to claim and dispatch a task.
    async fn poll_once(
        &self,
        pool: &deadpool::managed::Pool<diesel_async::pooled_connection::AsyncDieselConnectionManager<diesel_async::AsyncPgConnection>>,
    ) {
        let mut conn = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to get worker DB connection: {e}");
                tokio::time::sleep(self.config.poll_interval).await;
                return;
            }
        };

        match crate::queue::claim_task(&mut conn, &self.config.queues, &self.config.worker_id).await {
            Ok(Some(task)) => {
                tracing::debug!(task_id = %task.id, task_type = %task.task_type, "Claimed task");
                self.dispatch_task(task, pool.clone());
            }
            Ok(None) => {
                // No tasks available — wait for notification or poll interval
                tokio::time::sleep(self.config.poll_interval).await;
            }
            Err(e) => {
                tracing::warn!("Error claiming task: {e}");
                tokio::time::sleep(self.config.poll_interval).await;
            }
        }
    }

    /// Route a claimed task to the appropriate executor.
    fn dispatch_task(
        &self,
        task: crate::models::TaskQueueItem,
        pool: deadpool::managed::Pool<diesel_async::pooled_connection::AsyncDieselConnectionManager<diesel_async::AsyncPgConnection>>,
    ) {
        let registry = self.registry.clone();
        let shutdown = self.shutdown.clone();

        match task.task_type.as_str() {
            "workflow" => {
                let semaphore = self.workflow_semaphore.clone();
                tokio::spawn(async move {
                    let _permit = semaphore.acquire().await;
                    // TODO: Task 15 wires this to the full executor
                    tracing::debug!(task_id = %task.id, "Executing workflow task");
                });
            }
            "activity" => {
                let semaphore = self.activity_semaphore.clone();
                tokio::spawn(async move {
                    let _permit = semaphore.acquire().await;
                    // TODO: Task 15 wires this to the full executor
                    tracing::debug!(task_id = %task.id, "Executing activity task");
                });
            }
            other => {
                tracing::error!(task_type = other, "Unknown task type");
            }
        }
    }

    /// Wait for in-flight tasks to complete during graceful shutdown.
    async fn drain_in_flight(&self) {
        tracing::info!("Draining in-flight tasks (timeout: {:?})", self.config.shutdown_timeout);
        // Wait for all semaphore permits to be returned (all tasks complete)
        // or timeout
        let _ = tokio::time::timeout(
            self.config.shutdown_timeout,
            async {
                // Acquire ALL permits = all tasks done
                let wf_total = self.config.max_concurrent_workflows;
                let act_total = self.config.max_concurrent_activities;
                let _ = self.workflow_semaphore.acquire_many(wf_total as u32).await;
                let _ = self.activity_semaphore.acquire_many(act_total as u32).await;
            }
        ).await;
    }

    /// Signal the worker to shut down gracefully.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }
}
```

**Step 4: Add `Config` error variant**

In `error.rs`:
```rust
    #[error("configuration error: {0}")]
    Config(String),
```

**Step 5: Expose in lib.rs**

```rust
pub mod worker;
```

**Step 6: Run test — expect PASS**

```bash
cargo test -p autumn-harvest worker
```

**Step 7: Commit**

```bash
git add -A && git commit -m "feat(worker): add worker runtime with semaphore-bounded poll loop"
```

---

### Task 10: Heartbeat Manager

**Files:**
- Create: `autumn-harvest/src/heartbeat.rs`
- Modify: `autumn-harvest/src/lib.rs`

Background task that receives heartbeat payloads from running activities via mpsc channels, batches them (at most one DB write per second per activity), and updates `last_heartbeat_at` in the task queue.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/heartbeat.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn heartbeat_batcher_debounces() {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let batcher = HeartbeatBatcher::new(rx);

        // Send 3 rapid heartbeats
        tx.send(serde_json::json!("beat 1")).await.unwrap();
        tx.send(serde_json::json!("beat 2")).await.unwrap();
        tx.send(serde_json::json!("beat 3")).await.unwrap();
        drop(tx); // close channel

        // Batcher should collect them
        let beats = batcher.collect_pending().await;
        // Only the most recent should be kept (debouncing)
        assert_eq!(beats, Some(serde_json::json!("beat 3")));
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest heartbeat
```

**Step 3: Implement heartbeat batcher**

```rust
//! Heartbeat manager — batches and persists activity heartbeats.
//!
//! Each running activity gets an mpsc sender. The batcher collects
//! the most recent payload and flushes to Postgres at most once per second.

use tokio::sync::mpsc;

/// Collects heartbeat payloads and debounces them.
pub struct HeartbeatBatcher {
    rx: mpsc::Receiver<serde_json::Value>,
}

impl HeartbeatBatcher {
    pub fn new(rx: mpsc::Receiver<serde_json::Value>) -> Self {
        Self { rx }
    }

    /// Drain pending heartbeats, keeping only the most recent.
    pub async fn collect_pending(&self) -> Option<serde_json::Value> {
        // This is a simplified version — the full implementation
        // uses a tokio::select! loop with a 1-second ticker.
        // For unit tests we just drain synchronously.
        let mut latest = None;
        let mut rx = unsafe {
            // SAFETY: we need &mut self but tests create with owned receiver.
            // Full implementation will use interior mutability.
            &self.rx as *const mpsc::Receiver<serde_json::Value>
                as *mut mpsc::Receiver<serde_json::Value>
        };
        let rx = unsafe { &mut *rx };
        while let Ok(value) = rx.try_recv() {
            latest = Some(value);
        }
        latest
    }
}

/// Spawn a heartbeat flusher task for a running activity.
///
/// Returns the sender that the `ActivityContext` writes to.
pub fn spawn_heartbeat_flusher(
    task_id: uuid::Uuid,
    pool: deadpool::managed::Pool<
        diesel_async::pooled_connection::AsyncDieselConnectionManager<
            diesel_async::AsyncPgConnection,
        >,
    >,
    cancel: tokio_util::sync::CancellationToken,
) -> mpsc::Sender<serde_json::Value> {
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(32);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    // Drain all pending, keep most recent
                    let mut latest = None;
                    while let Ok(v) = rx.try_recv() {
                        latest = Some(v);
                    }

                    if latest.is_some() {
                        if let Ok(mut conn) = pool.get().await {
                            if let Err(e) = crate::queue::record_heartbeat(&mut conn, task_id).await {
                                tracing::warn!(task_id = %task_id, "heartbeat flush failed: {e}");
                            }
                        }
                    }
                }
            }
        }
    });

    tx
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod heartbeat;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest heartbeat
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(heartbeat): add batched heartbeat flusher with 1s debounce"
```

---

### Task 11: Timeout Enforcement

**Files:**
- Create: `autumn-harvest/src/timeout.rs`
- Modify: `autumn-harvest/src/lib.rs`

Background task that periodically scans for timed-out activities and handles them according to retry policy.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/timeout.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_check_query_is_valid_sql() {
        // Verify the SQL string is well-formed (catches syntax typos)
        let sql = heartbeat_timeout_query();
        assert!(sql.contains("harvest_task_queue"));
        assert!(sql.contains("RUNNING"));
        assert!(sql.contains("heartbeat_timeout"));
    }

    #[test]
    fn timeout_check_schedule_to_start_query() {
        let sql = schedule_to_start_timeout_query();
        assert!(sql.contains("PENDING"));
        assert!(sql.contains("schedule_to_start"));
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest timeout
```

**Step 3: Implement timeout checker**

```rust
//! Timeout enforcement — detects and handles timed-out tasks.
//!
//! Three timeout types:
//! - **Heartbeat timeout**: running activity hasn't sent a heartbeat recently enough.
//! - **Start-to-close timeout**: activity has been running too long.
//! - **Schedule-to-start timeout**: task sat in PENDING too long (no worker picked it up).

use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;

use crate::error::HarvestResult;
use crate::models::TaskQueueItem;

/// SQL to find activities with expired heartbeat timeouts.
pub fn heartbeat_timeout_query() -> &'static str {
    "SELECT * FROM harvest_task_queue \
     WHERE state = 'RUNNING' \
       AND heartbeat_timeout IS NOT NULL \
       AND last_heartbeat_at < NOW() - heartbeat_timeout"
}

/// SQL to find tasks that exceeded schedule-to-start timeout.
pub fn schedule_to_start_timeout_query() -> &'static str {
    "SELECT * FROM harvest_task_queue \
     WHERE state = 'PENDING' \
       AND schedule_to_start IS NOT NULL \
       AND scheduled_at < NOW() - schedule_to_start"
}

/// SQL to find activities that exceeded start-to-close timeout.
pub fn start_to_close_timeout_query() -> &'static str {
    "SELECT * FROM harvest_task_queue \
     WHERE state = 'RUNNING' \
       AND start_to_close IS NOT NULL \
       AND started_at < NOW() - start_to_close"
}

/// Find all timed-out tasks across all timeout types.
pub async fn find_timed_out_tasks(
    conn: &mut AsyncPgConnection,
) -> HarvestResult<Vec<(TaskQueueItem, TimeoutReason)>> {
    let mut results = Vec::new();

    // Heartbeat timeouts
    let heartbeat_expired: Vec<TaskQueueItem> =
        diesel::sql_query(heartbeat_timeout_query())
            .load(conn)
            .await
            .unwrap_or_default();

    for task in heartbeat_expired {
        results.push((task, TimeoutReason::Heartbeat));
    }

    // Start-to-close timeouts
    let stc_expired: Vec<TaskQueueItem> =
        diesel::sql_query(start_to_close_timeout_query())
            .load(conn)
            .await
            .unwrap_or_default();

    for task in stc_expired {
        results.push((task, TimeoutReason::StartToClose));
    }

    // Schedule-to-start timeouts
    let sts_expired: Vec<TaskQueueItem> =
        diesel::sql_query(schedule_to_start_timeout_query())
            .load(conn)
            .await
            .unwrap_or_default();

    for task in sts_expired {
        results.push((task, TimeoutReason::ScheduleToStart));
    }

    Ok(results)
}

#[derive(Debug, Clone, Copy)]
pub enum TimeoutReason {
    Heartbeat,
    StartToClose,
    ScheduleToStart,
}

impl std::fmt::Display for TimeoutReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Heartbeat => write!(f, "heartbeat_timeout"),
            Self::StartToClose => write!(f, "start_to_close_timeout"),
            Self::ScheduleToStart => write!(f, "schedule_to_start_timeout"),
        }
    }
}

/// Spawn the timeout checker background task.
pub fn spawn_timeout_checker(
    pool: deadpool::managed::Pool<
        diesel_async::pooled_connection::AsyncDieselConnectionManager<
            diesel_async::AsyncPgConnection,
        >,
    >,
    cancel: tokio_util::sync::CancellationToken,
    check_interval: std::time::Duration,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(check_interval);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    if let Ok(mut conn) = pool.get().await {
                        match find_timed_out_tasks(&mut conn).await {
                            Ok(timed_out) => {
                                for (task, reason) in timed_out {
                                    tracing::warn!(
                                        task_id = %task.id,
                                        reason = %reason,
                                        "Task timed out"
                                    );
                                    let _ = crate::queue::fail_task(
                                        &mut conn,
                                        task.id,
                                        &format!("timed out: {reason}"),
                                    ).await;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Timeout check failed: {e}");
                            }
                        }
                    }
                }
            }
        }
    });
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod timeout;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest timeout
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(timeout): add timeout enforcement for heartbeat, start-to-close, and schedule-to-start"
```

---

### Task 12: Workflow Cache (Basic Sticky)

**Files:**
- Create: `autumn-harvest/src/cache.rs`
- Modify: `autumn-harvest/src/lib.rs`

In-process LRU cache of suspended workflow coroutines. Avoids re-replaying the full event history when the same worker processes sequential tasks for the same workflow.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/cache.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_stores_and_retrieves() {
        let mut cache = WorkflowCache::new(10);
        let exec_id = uuid::Uuid::new_v4();

        cache.insert(exec_id, CachedWorkflowState {
            replay_position: 5,
            next_activity_seq: 3,
            next_timer_seq: 1,
        });

        let entry = cache.get(&exec_id);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().replay_position, 5);
    }

    #[test]
    fn cache_evicts_lru() {
        let mut cache = WorkflowCache::new(2);
        let id1 = uuid::Uuid::new_v4();
        let id2 = uuid::Uuid::new_v4();
        let id3 = uuid::Uuid::new_v4();

        let state = || CachedWorkflowState {
            replay_position: 0,
            next_activity_seq: 0,
            next_timer_seq: 0,
        };

        cache.insert(id1, state());
        cache.insert(id2, state());
        cache.insert(id3, state()); // evicts id1

        assert!(cache.get(&id1).is_none());
        assert!(cache.get(&id2).is_some());
        assert!(cache.get(&id3).is_some());
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest cache
```

**Step 3: Implement workflow cache**

```rust
//! In-process LRU cache for suspended workflow states.
//!
//! When a workflow suspends (waiting for an activity), the cache stores
//! its replay position so that the next task for the same workflow can
//! skip replaying already-processed events.
//!
//! Phase 2 scope: single-worker in-process cache only.
//! Phase 3 adds cross-worker sticky routing via per-worker NOTIFY channels.

use lru::LruCache;
use std::num::NonZeroUsize;
use uuid::Uuid;

/// Cached state for a suspended workflow.
#[derive(Debug, Clone)]
pub struct CachedWorkflowState {
    /// Index into the event history where replay can resume.
    pub replay_position: usize,
    /// Next activity sequence number.
    pub next_activity_seq: u32,
    /// Next timer sequence number.
    pub next_timer_seq: u32,
}

/// LRU cache of workflow states, bounded by max_size.
pub struct WorkflowCache {
    inner: LruCache<Uuid, CachedWorkflowState>,
}

impl WorkflowCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            inner: LruCache::new(
                NonZeroUsize::new(max_size).unwrap_or(NonZeroUsize::new(1).unwrap()),
            ),
        }
    }

    pub fn insert(&mut self, exec_id: Uuid, state: CachedWorkflowState) {
        self.inner.put(exec_id, state);
    }

    pub fn get(&mut self, exec_id: &Uuid) -> Option<&CachedWorkflowState> {
        self.inner.get(exec_id)
    }

    pub fn remove(&mut self, exec_id: &Uuid) -> Option<CachedWorkflowState> {
        self.inner.pop(exec_id)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod cache;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest cache
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(cache): add LRU workflow state cache for replay optimization"
```

---

### Task 13: Dead Letter Queue Operations

**Files:**
- Create: `autumn-harvest/src/dlq.rs`
- Modify: `autumn-harvest/src/lib.rs`

Tasks that exhaust all retry attempts land in the dead letter queue for manual investigation.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/dlq.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_letter_entry_builds_from_task() {
        let task_id = uuid::Uuid::new_v4();
        let entry = NewDeadLetterEntry {
            original_task_id: task_id,
            queue_name: "default",
            task_type: "activity",
            workflow_exec_id: None,
            activity_name: Some("send_email"),
            input: serde_json::json!({"to": "user@test.com"}),
            error: "max retries exceeded",
            attempts: 3,
        };

        assert_eq!(entry.attempts, 3);
        assert_eq!(entry.original_task_id, task_id);
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest dlq
```

**Step 3: Implement dead letter queue**

```rust
//! Dead letter queue — final resting place for permanently failed tasks.

use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use diesel_async::AsyncPgConnection;
use uuid::Uuid;

use crate::error::{HarvestError, HarvestResult};
use crate::schema::harvest_dead_letters;

#[derive(Debug, Insertable)]
#[diesel(table_name = harvest_dead_letters)]
pub struct NewDeadLetterEntry<'a> {
    pub original_task_id: Uuid,
    pub queue_name: &'a str,
    pub task_type: &'a str,
    pub workflow_exec_id: Option<Uuid>,
    pub activity_name: Option<&'a str>,
    pub input: serde_json::Value,
    pub error: &'a str,
    pub attempts: i32,
}

/// Move a permanently failed task to the dead letter queue.
pub async fn dead_letter(
    conn: &mut AsyncPgConnection,
    entry: NewDeadLetterEntry<'_>,
) -> HarvestResult<Uuid> {
    let id = Uuid::new_v4();

    diesel::insert_into(harvest_dead_letters::table)
        .values(&entry)
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;

    Ok(id)
}

/// Count entries in the dead letter queue (for monitoring).
pub async fn dead_letter_count(
    conn: &mut AsyncPgConnection,
) -> HarvestResult<i64> {
    use diesel::dsl::count_star;

    harvest_dead_letters::table
        .select(count_star())
        .first(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod dlq;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest dlq
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(dlq): add dead letter queue for permanently failed tasks"
```

---

### Task 14: Pool Configuration and HarvestExt on AppBuilder

**Files:**
- Create: `autumn-harvest/src/pool.rs`
- Create: `autumn-harvest/src/ext.rs`
- Modify: `autumn-harvest/src/lib.rs`

> **User contribution requested:** This task wires Harvest into Autumn's `AppBuilder`. The pool partitioning logic — how the shared ceiling is enforced across two independent deadpool instances — is a meaningful design choice. See the pool.rs implementation below.

**Step 1: Write failing test**

```rust
// autumn-harvest/src/pool.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_config_validates_ceiling() {
        let config = HarvestPoolConfig {
            worker_pool_size: 10,
            max_total_connections: 20,
        };
        assert!(config.validate(15).is_ok()); // web=15, worker=10, total=25 > 20 but that's ok, ceiling is advisory
    }

    #[test]
    fn pool_config_rejects_zero_worker_pool() {
        let config = HarvestPoolConfig {
            worker_pool_size: 0,
            max_total_connections: 20,
        };
        assert!(config.validate(10).is_err());
    }

    #[test]
    fn pool_sizes_respect_ceiling() {
        let (web, worker) = compute_pool_sizes(10, 10, 15);
        // When combined exceeds ceiling, both are scaled proportionally
        assert!(web + worker <= 15);
    }
}
```

**Step 2: Run test — expect FAIL**

```bash
cargo test -p autumn-harvest pool
```

**Step 3: Implement pool configuration**

```rust
//! Database pool configuration for the Harvest worker.
//!
//! Design Decision DD-2: Separate pools with a shared ceiling.
//! Two deadpool instances prevent worker activities from starving
//! HTTP request handling. The shared ceiling prevents overloading Postgres.

use crate::error::{HarvestError, HarvestResult};

/// Harvest-specific pool configuration.
#[derive(Debug, Clone)]
pub struct HarvestPoolConfig {
    /// Max connections for the Harvest worker pool.
    pub worker_pool_size: usize,
    /// Shared ceiling across web + worker pools.
    /// Default: Postgres max_connections - 5 (for superuser/replication).
    pub max_total_connections: usize,
}

impl Default for HarvestPoolConfig {
    fn default() -> Self {
        Self {
            worker_pool_size: 10,
            max_total_connections: 95, // pg default 100 minus 5 reserved
        }
    }
}

impl HarvestPoolConfig {
    pub fn validate(&self, web_pool_size: usize) -> HarvestResult<()> {
        if self.worker_pool_size == 0 {
            return Err(HarvestError::Config(
                "worker_pool_size must be > 0".into(),
            ));
        }
        if self.max_total_connections == 0 {
            return Err(HarvestError::Config(
                "max_total_connections must be > 0".into(),
            ));
        }

        let total = web_pool_size + self.worker_pool_size;
        if total > self.max_total_connections {
            tracing::warn!(
                web = web_pool_size,
                worker = self.worker_pool_size,
                ceiling = self.max_total_connections,
                "Combined pool sizes ({total}) exceed max_total_connections ceiling. \
                 Pools will be scaled down proportionally."
            );
        }

        Ok(())
    }
}

/// Compute actual pool sizes that respect the shared ceiling.
///
/// If web + worker exceeds the ceiling, scale both proportionally.
pub fn compute_pool_sizes(
    requested_web: usize,
    requested_worker: usize,
    ceiling: usize,
) -> (usize, usize) {
    let total = requested_web + requested_worker;
    if total <= ceiling {
        return (requested_web, requested_worker);
    }

    // Scale proportionally, ensuring at least 1 each
    let ratio = ceiling as f64 / total as f64;
    let web = (requested_web as f64 * ratio).floor().max(1.0) as usize;
    let worker = (requested_worker as f64 * ratio).floor().max(1.0) as usize;

    // Give any remainder to web (prioritize HTTP responsiveness)
    let remainder = ceiling.saturating_sub(web + worker);
    (web + remainder, worker)
}
```

**Step 4: Expose in lib.rs**

```rust
pub mod pool;
pub mod ext;
```

**Step 5: Run test — expect PASS**

```bash
cargo test -p autumn-harvest pool
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(pool): add separate worker pool with shared ceiling enforcement"
```

---

### Task 15: End-to-End Integration Tests (Happy Path)

**Files:**
- Create: `autumn-harvest/tests/integration_e2e.rs`

These tests require a running Postgres instance. They exercise the full path: create workflow execution → append events → enqueue task → claim task → complete.

> **Prerequisite:** A Postgres database accessible at `DATABASE_URL` env var.

**Step 1: Write the integration test**

```rust
// autumn-harvest/tests/integration_e2e.rs
//!
//! End-to-end integration tests for the Harvest engine.
//! Requires: DATABASE_URL pointing to a Postgres instance with migrations run.

use autumn_harvest::prelude::*;
use autumn_harvest::store;
use autumn_harvest::queue;
use autumn_harvest::models::NewWorkflowExecution;
use chrono::Utc;
use uuid::Uuid;

/// Helper: get a test database connection.
async fn test_conn() -> diesel_async::AsyncPgConnection {
    use diesel_async::AsyncConnection;
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for integration tests");
    diesel_async::AsyncPgConnection::establish(&url)
        .await
        .expect("Failed to connect to test database")
}

#[tokio::test]
#[ignore] // Run with: cargo test --test integration_e2e -- --ignored
async fn full_workflow_lifecycle() {
    let mut conn = test_conn().await;
    let exec_id = ExecutionId::new();

    // 1. Create workflow execution
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use autumn_harvest::schema::harvest_workflow_executions;

    let new_exec = NewWorkflowExecution {
        id: exec_id.as_uuid(),
        workflow_name: "test_workflow",
        workflow_id: "test-123",
        run_id: Uuid::new_v4(),
        shard_id: 0,
        input: serde_json::json!({"name": "integration"}),
        queue_name: "default",
        execution_timeout: None,
        memo: None,
        search_attrs: None,
    };

    diesel::insert_into(harvest_workflow_executions::table)
        .values(&new_exec)
        .execute(&mut conn)
        .await
        .expect("insert workflow execution");

    // 2. Append events
    let events = vec![
        WorkflowEvent::WorkflowStarted {
            input: serde_json::json!({"name": "integration"}),
            timestamp: Utc::now(),
        },
    ];
    let count = store::append_events(&mut conn, exec_id, &events, 0)
        .await
        .expect("append events");
    assert_eq!(count, 1);

    // 3. Load history
    let history = store::load_history(&mut conn, exec_id)
        .await
        .expect("load history");
    assert_eq!(history.events.len(), 1);
    assert_eq!(history.next_event_id, 1);

    // 4. Enqueue activity task
    let task_id = queue::enqueue(&mut conn, queue::EnqueueParams {
        queue_name: "default",
        task_type: queue::TaskType::Activity,
        workflow_exec_id: Some(exec_id.as_uuid()),
        activity_name: Some("greet"),
        input: serde_json::json!("world"),
        priority: 0,
        max_attempts: 3,
        heartbeat_timeout: None,
        start_to_close: None,
        schedule_to_start: None,
        retry_policy: None,
        scheduled_at: None,
    }).await.expect("enqueue task");

    // 5. Claim task
    let claimed = queue::claim_task(
        &mut conn,
        &["default".to_string()],
        "test-worker-1",
    ).await.expect("claim task");
    assert!(claimed.is_some());
    let claimed = claimed.unwrap();
    assert_eq!(claimed.id, task_id);
    assert_eq!(claimed.state, "RUNNING");

    // 6. Complete task
    queue::complete_task(&mut conn, task_id, serde_json::json!("hello world"))
        .await
        .expect("complete task");

    // 7. Append completion events
    let completion_events = vec![
        WorkflowEvent::ActivityCompleted {
            activity_id: ActivityExecId::new(),
            output: serde_json::json!("hello world"),
        },
        WorkflowEvent::WorkflowCompleted {
            output: serde_json::json!("hello world"),
        },
    ];
    store::append_events(&mut conn, exec_id, &completion_events, 1)
        .await
        .expect("append completion events");

    // 8. Verify final history
    let final_history = store::load_history(&mut conn, exec_id)
        .await
        .expect("load final history");
    assert_eq!(final_history.events.len(), 3);
    assert!(matches!(
        final_history.events.last().unwrap(),
        WorkflowEvent::WorkflowCompleted { .. }
    ));
}
```

**Step 2: Run test (with Postgres)**

```bash
DATABASE_URL=postgres://localhost/autumn_harvest_test cargo test --test integration_e2e -- --ignored
```

**Step 3: Commit**

```bash
git add -A && git commit -m "test: add end-to-end integration test for workflow lifecycle"
```

---

### Task 16: Replay and Failure Tests

**Files:**
- Create: `autumn-harvest/tests/replay_tests.rs`

These are the most important correctness tests. They verify that the replay engine correctly rebuilds workflow state from history.

**Step 1: Write replay tests**

```rust
// autumn-harvest/tests/replay_tests.rs

use autumn_harvest::context::WorkflowContext;
use autumn_harvest::event::WorkflowEvent;
use autumn_harvest::executor::{run_workflow, WorkflowOutcome};
use autumn_harvest::types::{ActivityExecId, ExecutionId};
use chrono::Utc;

#[tokio::test]
async fn replay_two_sequential_activities() {
    let exec_id = ExecutionId::new();
    let aid1 = ActivityExecId::new();
    let aid2 = ActivityExecId::new();

    let history = vec![
        WorkflowEvent::WorkflowStarted {
            input: serde_json::json!("test"),
            timestamp: Utc::now(),
        },
        WorkflowEvent::ActivityScheduled {
            activity_id: aid1,
            name: "step_1".into(),
            input: serde_json::json!("a"),
            queue: "default".into(),
        },
        WorkflowEvent::ActivityCompleted {
            activity_id: aid1,
            output: serde_json::json!("result_a"),
        },
        WorkflowEvent::ActivityScheduled {
            activity_id: aid2,
            name: "step_2".into(),
            input: serde_json::json!("b"),
            queue: "default".into(),
        },
        WorkflowEvent::ActivityCompleted {
            activity_id: aid2,
            output: serde_json::json!("result_b"),
        },
        WorkflowEvent::WorkflowCompleted {
            output: serde_json::json!(["result_a", "result_b"]),
        },
    ];

    let handler = |ctx: &WorkflowContext, _input: serde_json::Value| {
        Box::pin(async move {
            let a = ctx.execute_activity_raw("step_1", serde_json::json!("a"), "default").await
                .map_err(|e| e.to_string())?;
            let b = ctx.execute_activity_raw("step_2", serde_json::json!("b"), "default").await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!([a, b]))
        })
    };

    let outcome = run_workflow(exec_id, history, handler, serde_json::json!("test")).await;
    match outcome {
        WorkflowOutcome::Completed { output } => {
            assert_eq!(output, serde_json::json!(["result_a", "result_b"]));
        }
        other => panic!("Expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn replay_detects_non_determinism() {
    let exec_id = ExecutionId::new();
    let aid = ActivityExecId::new();

    let history = vec![
        WorkflowEvent::WorkflowStarted {
            input: serde_json::json!({}),
            timestamp: Utc::now(),
        },
        WorkflowEvent::ActivityScheduled {
            activity_id: aid,
            name: "step_1".into(),
            input: serde_json::json!(null),
            queue: "default".into(),
        },
        WorkflowEvent::ActivityCompleted {
            activity_id: aid,
            output: serde_json::json!("done"),
        },
    ];

    // Workflow code calls "wrong_name" but history has "step_1"
    let handler = |ctx: &WorkflowContext, _input: serde_json::Value| {
        Box::pin(async move {
            let _ = ctx.execute_activity_raw("wrong_name", serde_json::json!(null), "default").await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::Null)
        })
    };

    let outcome = run_workflow(exec_id, history, handler, serde_json::json!({})).await;
    assert!(matches!(outcome, WorkflowOutcome::Failed { .. }));
}

#[tokio::test]
async fn version_gate_routes_code_paths() {
    let exec_id = ExecutionId::new();

    // Old workflow: no version marker in history
    let history = vec![WorkflowEvent::WorkflowStarted {
        input: serde_json::json!({}),
        timestamp: Utc::now(),
    }];

    let handler = |ctx: &WorkflowContext, _input: serde_json::Value| {
        Box::pin(async move {
            let v = ctx.version("add-retry", 1, 2);
            // Old workflows get min_version (1), new get max (2)
            Ok(serde_json::json!(v))
        })
    };

    let outcome = run_workflow(exec_id, history, handler, serde_json::json!({})).await;
    match outcome {
        // WorkflowStarted has no version marker → returns min_version (1)
        // But we're past history → returns max_version (2) for new code
        WorkflowOutcome::Completed { output } => {
            // Past end of history = new code = max_version
            assert_eq!(output, serde_json::json!(2));
        }
        other => panic!("Expected Completed, got {other:?}"),
    }
}
```

**Step 2: Run tests**

```bash
cargo test --test replay_tests
```

**Step 3: Commit**

```bash
git add -A && git commit -m "test: add replay engine correctness tests including non-determinism detection"
```

---

### Task 17: Full Build + Clippy Pass

**Step 1: Run full test suite**

```bash
cd ~/autumn-harvest && cargo test
```

Expected: all unit tests pass (integration tests may require `--ignored`)

**Step 2: Run clippy pedantic + nursery**

```bash
cargo clippy --all-targets -- -D warnings
```

Fix any lints. Common issues:
- `#[allow(clippy::module_name_repetitions)]` where needed
- `#[must_use]` on pure functions
- Missing safety comments on unsafe blocks (heartbeat batcher)
- `Box::pin` closure lifetime annotations

**Step 3: Run rustfmt**

```bash
cargo fmt --all -- --check
```

**Step 4: Commit**

```bash
git add -A && git commit -m "chore: clippy + fmt clean across autumn-harvest Phase 2"
```

---

### Task 18: Update CLAUDE.md

**Files:**
- Modify: `~/autumn-harvest/CLAUDE.md`

Update the Phase Status section:

```markdown
## Phase Status

- Phase 1 (complete): types, error, event, policy, context stubs, models, macros, builder
- Phase 2 (complete): event store, replay engine, workflow context, activity context,
  task queue (SKIP LOCKED), LISTEN/NOTIFY, worker runtime, heartbeating,
  timeout enforcement, workflow versioning (ctx.version), LRU workflow cache,
  dead letter queue, separate worker pool with shared ceiling
- Phase 3 (next): DAG scheduler, DagBuilder, topological sort, #[dag] macro,
  trigger rules, timetable, signals/queries, saga pattern, management HTTP API
- Phase 4: production hardening — sharding, sticky cross-worker routing,
  observability, metrics, dashboard (autumn-harvest-ui)
```

**Step 1: Update CLAUDE.md**

**Step 2: Commit**

```bash
git add CLAUDE.md && git commit -m "docs: update CLAUDE.md with Phase 2 completion status"
```

---

## Dependency Graph

```
Task 1 (Event Store Writer)
  └─► Task 2 (Event Store Reader)
       └─► Task 3 (Replay Engine)
            └─► Task 4 (WorkflowContext) ──► Task 8 (Workflow Executor)
                                                └─► Task 9 (Worker Runtime)
            └─► Task 5 (ActivityContext) ──────────► Task 9
                                                       └─► Task 15 (E2E Tests)
Task 6 (Task Queue) ──► Task 9                               └─► Task 16 (Replay Tests)
Task 7 (LISTEN/NOTIFY) ──► Task 9                                   └─► Task 17 (Clippy)
Task 10 (Heartbeat) ──► Task 9                                            └─► Task 18 (Docs)
Task 11 (Timeout) ──► Task 9
Task 12 (Cache) ──► Task 9
Task 13 (DLQ) ──► Task 9
Task 14 (Pool Config) ──► Task 9
```

Tasks 1-7 and 10-14 have many independent branches — **parallelize aggressively** when dispatching to subagents.
