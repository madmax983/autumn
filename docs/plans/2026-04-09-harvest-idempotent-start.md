# Harvest Idempotent Start Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make duplicate workflow starts with the same business key converge on one Harvest execution instead of creating duplicate rows and queue work.

**Architecture:** Treat `workflow_id` as the caller-provided idempotency key within a `workflow_name`. Enforce that contract in Postgres with a unique constraint, then funnel both the Harvest HTTP API and app-side publication helpers through one shared start-or-load helper that returns an existing execution when a duplicate start races or retries.

**Tech Stack:** Rust, Diesel Async, Postgres, testcontainers, Axum

---

### Task 1: Capture Duplicate-Start Behavior With Failing Tests

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/tests/api_scheduler_integration.rs`
- Modify: `examples/reddit-clone/src/workflows.rs`

**Step 1: Write the failing API test**

Add an integration test that starts the same workflow twice with the same `workflow_id`, then asserts:
- the second call does not create a second execution
- both responses carry the same `execution_id`
- only one workflow row exists for `(workflow_name, workflow_id)`
- only one queued workflow task exists for that execution

**Step 2: Run the API test to verify it fails**

Run: `cargo test -p autumn-web-harvest --test api_scheduler_integration harvest_api_duplicate_start_reuses_existing_execution`

Expected: FAIL because the current start path blindly inserts a new execution every time.

**Step 3: Write the failing reddit-clone helper test**

Add a focused test around the app-side publication helper so repeated publication with the same derived `workflow_id` resolves to one execution instead of duplicating queue work.

**Step 4: Run the helper test to verify it fails**

Run: `cargo test -p reddit-clone workflows`

Expected: FAIL on the duplicate publication case.

### Task 2: Add Shared Harvest Start-Or-Load Helper

**Files:**
- Create: `autumn-harvest/autumn-harvest/src/execution.rs`
- Modify: `autumn-harvest/autumn-harvest/src/lib.rs`
- Modify: `autumn-harvest/autumn-harvest/src/prelude.rs` if the helper should be re-exported

**Step 1: Write the helper surface**

Add a shared DB helper that:
- accepts workflow identity, queue, input, memo, search attrs, and timeout
- attempts to insert the workflow execution, start event, and queue task transactionally
- on uniqueness conflict, loads the existing execution by `(workflow_name, workflow_id)` and returns it

**Step 2: Keep the helper narrow**

Do not implement outbox logic here. This slice is only “duplicate start is safe,” not “cross-store delivery is magically atomic.”

### Task 3: Enforce Idempotency In The Schema

**Files:**
- Modify: `autumn-harvest/autumn-harvest/migrations/20260409000000_harvest_initial/up.sql`
- Modify: `autumn-harvest/autumn-harvest/src/types.rs`

**Step 1: Add the real uniqueness boundary**

Change the workflow execution schema so `(workflow_name, workflow_id)` is unique.

**Step 2: Update the type docs**

Make `WorkflowId` documentation match reality: it is the caller’s idempotency key for a logical workflow start. Fresh reruns should use a fresh key until Harvest grows an explicit restart/rerun API.

### Task 4: Route All Start Paths Through The Shared Helper

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/api.rs`
- Modify: `examples/reddit-clone/src/workflows.rs`

**Step 1: Update the Harvest API**

Use the new helper in `start_workflow`. Return:
- `201 Created` when a new execution is created
- `200 OK` when a duplicate start reuses an existing execution

**Step 2: Update reddit-clone publication**

Replace the local blind insert helper with the shared Harvest helper so split-mode app publication gets the same idempotent behavior as the HTTP API.

### Task 5: Verify And Document

**Files:**
- Modify: `docs/plans/2026-04-09-harvest-topology-roadmap.md` only if the implementation meaningfully changes the wording

**Step 1: Run focused verification**

Run:
- `cargo fmt --all`
- `cargo test -p autumn-harvest --lib`
- `cargo test -p autumn-web-harvest --test api_scheduler_integration`
- `cargo test -p reddit-clone workflows`

**Step 2: Run broader compile verification**

Run:
- `cargo test -p reddit-clone --no-run -j 1`

**Step 3: Record any remaining caveat honestly**

The remaining caveat should now be “duplicate starts are safe, but split-store publication is still not atomic without outbox semantics.”
