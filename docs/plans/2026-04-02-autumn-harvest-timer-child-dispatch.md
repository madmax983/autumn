# Timer And Child Dispatch Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add real worker support for durable timers and child workflow dispatch so suspended workflows can resume when timers fire or child executions reach a terminal state.

**Architecture:** Extend the same Phase 3 runtime seam used for activities. Timers will persist `TimerStarted`, register a durable timer row, and reschedule the parked workflow task for the fire time; workflow polling will ingest due timers into `TimerFired` events before replay. Child workflows will persist `ChildWorkflowStarted`, create a child execution plus workflow task, and append `ChildWorkflowCompleted` or `ChildWorkflowFailed` back to the parent when the child finishes, then wake the parked parent task.

**Tech Stack:** Rust, Tokio, Diesel Async, PostgreSQL event history + queue tables, testcontainers integration tests.

---

### Task 1: Add RED integration tests

**Files:**
- Modify: `autumn-harvest/autumn-harvest/tests/integration_e2e.rs`

**Step 1: Write the failing timer round-trip test**
- Add a workflow that calls `ctx.timer("cooldown", 1).await` and then returns a JSON payload.
- Add a DB-backed integration test asserting:
  - the execution reaches `COMPLETED`
  - parent history includes `WorkflowStarted`, `TimerStarted`, `TimerFired`, `WorkflowCompleted`
  - the timer row exists and is marked fired

**Step 2: Run the timer test to verify it fails**
- Run: `cargo test -p autumn-harvest --test integration_e2e worker_completes_workflow_with_timer_round_trip -- --exact`
- Expected: FAIL because `StartTimer` is still unsupported in the worker.

**Step 3: Write the failing child-workflow round-trip test**
- Add a parent workflow that awaits `spawn_child_workflow_raw("child_echo_workflow", input)`.
- Add a child workflow that returns deterministic output.
- Add a DB-backed integration test asserting:
  - parent reaches `COMPLETED`
  - parent history includes `ChildWorkflowStarted`, `ChildWorkflowCompleted`, `WorkflowCompleted`
  - child execution exists, completes, and records its own `WorkflowStarted` + `WorkflowCompleted`

**Step 4: Run the child test to verify it fails**
- Run: `cargo test -p autumn-harvest --test integration_e2e worker_completes_parent_workflow_after_child_workflow_round_trip -- --exact`
- Expected: FAIL because `StartChildWorkflow` is still unsupported in the worker.

### Task 2: Implement durable timer scheduling and ingestion

**Files:**
- Modify: `autumn-harvest/autumn-harvest/src/worker.rs`
- Modify: `autumn-harvest/autumn-harvest/src/queue.rs`

**Step 1: Add worker helpers for timer commands**
- Extract a `StartedTimerCommand` from a marker-plus-timer suspended command set.
- Persist `TimerStarted` plus any marker events in one transaction.
- Insert a `harvest_timers` row with `fires_at`.

**Step 2: Reschedule the workflow task for the fire time**
- Add a queue helper that sets a claimed workflow task back to `PENDING` at an explicit timestamp.
- Use that helper when parking a workflow on a timer.

**Step 3: Ingest fired timers before replay**
- Query `harvest_timers` for due, unfired rows belonging to the current workflow execution.
- Append `TimerFired` events and mark those timer rows fired before rerunning replay.

**Step 4: Run the timer test and make it green**
- Run: `cargo test -p autumn-harvest --test integration_e2e worker_completes_workflow_with_timer_round_trip -- --exact`

### Task 3: Implement child workflow dispatch and parent wake-up

**Files:**
- Modify: `autumn-harvest/autumn-harvest/src/worker.rs`
- Modify: `autumn-harvest/autumn-harvest/src/models.rs` only if a cleaner insert path needs new fields

**Step 1: Add worker helpers for child workflow commands**
- Extract a `StartedChildWorkflowCommand` from a marker-plus-child suspended command set.
- Persist `ChildWorkflowStarted` plus any marker events in the parent history.

**Step 2: Create and enqueue the child execution**
- Insert a child `harvest_workflow_executions` row linked to the parent.
- Append child `WorkflowStarted`.
- Enqueue a workflow task for the child execution.

**Step 3: Propagate child terminal state back to the parent**
- When a workflow execution with `parent_id` completes or fails, append `ChildWorkflowCompleted` or `ChildWorkflowFailed` to the parent history and wake the parked parent workflow task.

**Step 4: Run the child test and make it green**
- Run: `cargo test -p autumn-harvest --test integration_e2e worker_completes_parent_workflow_after_child_workflow_round_trip -- --exact`

### Task 4: Verify the full slice

**Files:**
- Modify if needed: `autumn-harvest/autumn-harvest/src/worker.rs`
- Modify if needed: `autumn-harvest/autumn-harvest/tests/integration_e2e.rs`

**Step 1: Run focused verification**
- `cargo test -p autumn-harvest --test integration_e2e worker_completes_workflow_with_timer_round_trip -- --exact`
- `cargo test -p autumn-harvest --test integration_e2e worker_completes_parent_workflow_after_child_workflow_round_trip -- --exact`

**Step 2: Run broader verification**
- `cargo test -p autumn-harvest --no-default-features`
- `cargo test -p autumn-harvest --test integration_e2e`

**Step 3: Check formatting**
- `cargo fmt --all`
