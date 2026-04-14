# Autumn Harvest Runtime Dispatch Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace Harvest's remaining worker dispatch stub with real workflow suspension handling and activity task execution.

**Architecture:** Keep the existing replay model. When a workflow suspends on `ScheduleActivity`, persist marker and scheduling events, enqueue an activity task, and leave the workflow task parked in `RUNNING`. When the activity reaches a terminal result, append its terminal event, complete or fail the activity task, and wake the parked workflow task so replay can continue. Unsupported suspension commands still fail explicitly.

**Tech Stack:** Rust 1.86+, Tokio, Diesel 2 + diesel-async, deadpool, Postgres, testcontainers, serde/serde_json.

---

### Task 1: Add Red Tests For Real Activity Dispatch

**Files:**
- Modify: `autumn-harvest/autumn-harvest/tests/integration_e2e.rs`

**Step 1: Write the failing test**

Add an integration test proving:
- a workflow task that suspends on `execute_activity_raw(...)` no longer fails immediately
- an activity task is claimed and executed by the worker
- the workflow later resumes and completes with the activity output

Also update the old unimplemented-dispatch assertion so an orphaned activity task now fails for a precise reason: no matching scheduled activity event exists.

**Step 2: Run the targeted test to verify it fails**

Run: `cargo test -p autumn-harvest --test integration_e2e worker_completes_workflow_with_activity_round_trip -- --exact`

Expected: FAIL because the worker still turns suspended workflow commands into `"activity/timer dispatch is not implemented yet"` or fails activity tasks as unimplemented.

### Task 2: Implement Workflow Suspension Persistence For Activity Scheduling

**Files:**
- Modify: `autumn-harvest/autumn-harvest/src/worker.rs`
- Modify: `autumn-harvest/autumn-harvest/src/queue.rs`

**Step 1: Add queue helper to wake parked workflow tasks**

Add a helper that finds the parked `workflow` task for a `workflow_exec_id` and resets it to `PENDING` so replay can continue after the activity reaches a terminal state.

**Step 2: Implement supported suspended-command handling**

In `worker.rs`, support this suspended workflow shape:
- zero or more `RecordMarker`
- exactly one `ScheduleActivity`

Persist marker events first, then `ActivityScheduled`, then enqueue the activity task with defaults from `ActivityInfo`.

Keep the existing signal-wait requeue path, but make it persist pending marker events before requeueing.

**Step 3: Run the targeted test**

Run: `cargo test -p autumn-harvest --test integration_e2e worker_completes_workflow_with_activity_round_trip -- --exact`

Expected: still FAIL because activity task execution is not wired yet.

### Task 3: Implement Real Activity Task Execution

**Files:**
- Modify: `autumn-harvest/autumn-harvest/src/worker.rs`

**Step 1: Resolve scheduled activity identity from workflow history**

Find the unmatched `ActivityScheduled` event for the task's execution and activity name. Use that event's `activity_id` for `ActivityStarted`, `ActivityCompleted`, and final `ActivityFailed`.

**Step 2: Execute the activity handler**

Use the registered `ActivityInfo` handler with an `ActivityContext`, optional heartbeat flusher, and cancellation token.

**Step 3: Persist terminal result and wake workflow**

On success:
- append `ActivityStarted` and `ActivityCompleted`
- complete the activity task
- wake the parked workflow task

On failure with retries remaining:
- requeue only the activity task with computed delay
- do not append terminal failure yet

On final failure:
- append `ActivityFailed`
- fail the activity task
- wake the parked workflow task so replay can observe the failure

**Step 4: Run the targeted tests**

Run:
- `cargo test -p autumn-harvest --test integration_e2e worker_completes_workflow_with_activity_round_trip -- --exact`
- `cargo test -p autumn-harvest --test integration_e2e worker_fails_orphaned_activity_task_without_scheduled_event -- --exact`

Expected: PASS

### Task 4: Run Focused Non-DB Verification

**Files:**
- Modify as needed for lint/doc fallout:
  - `autumn-harvest/autumn-harvest/src/context.rs`
  - `autumn-harvest/autumn-harvest/src/dag.rs`
  - `autumn-harvest/autumn-harvest/src/info.rs`
  - `autumn-harvest/autumn-harvest/src/query.rs`
  - `autumn-harvest/autumn-harvest/src/replay.rs`

**Step 1: Run verification**

Run:
- `cargo test -p autumn-harvest --no-default-features`
- `cargo fmt --check`
- `cargo clippy -p autumn-harvest --no-default-features -- -D warnings`

**Step 2: Fix fallout minimally**

Only address issues directly caused by this slice plus already-known no-default clippy/doc drift needed to keep the quality gate honest.
