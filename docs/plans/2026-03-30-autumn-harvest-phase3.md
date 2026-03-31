# Autumn Harvest Phase 3 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Deliver the next real Harvest milestone: DAG definitions and scheduling, workflow signals and queries, saga compensation, and the management HTTP API.

**Architecture:** Phase 2 gave Harvest its runtime spine, but there is still one fake rib: `worker.rs` contains a stubbed dispatch path. Phase 3 starts by removing that lie, then adds a DAG definition layer (`DagBuilder`, `DagInfo`, `#[dag]`, `dags![]`), a scheduler loop backed by `harvest_schedules` and `harvest_dag_runs`, signal/query runtime primitives on `WorkflowContext`, a small but durable saga helper, and Axum routes mounted through `HarvestExt`. Keep the verified core small: pure DAG graph building, topological sort, trigger-rule evaluation, and signal/query registry logic should stay unit-testable without Postgres. Use DB-backed integration tests only for queueing, scheduler persistence, and HTTP/API wiring.

**Tech Stack:** Rust 1.86+, edition 2024, Tokio, Axum, Diesel 2 + diesel-async, deadpool, Postgres (`LISTEN/NOTIFY`, advisory locks, scheduler tables), serde/serde_json, uuid, chrono, tokio-cron-scheduler or existing cron parser already in the workspace if available.

**Depends on:** Phase 2 runtime modules already present in `C:\Users\markm\autumn\autumn-harvest`, especially `queue.rs`, `store.rs`, `executor.rs`, `worker.rs`, `models.rs`, and `schema.rs`.

---

## Design Decisions Baked Into This Plan

### DD-1: Fix The Worker Stub Before Layering Phase 3

`C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\worker.rs` still completes claimed tasks with `null`. Phase 3 features that depend on real workflow advancement must not stack on that placeholder. Task dispatch must execute workflows/activities for real before any DAG scheduler or signal delivery code lands.

### DD-2: DAGs Compile To Immutable Metadata, Not Dynamic Graph Mutation

The `#[dag]` macro should emit `DagInfo` companion metadata the same way `#[workflow]` and `#[activity]` already do. Runtime scheduling consumes pre-built DAG metadata; it should never build edges from strings at runtime.

### DD-3: Keep DAG Graph Logic Pure

Topological sort, cycle detection, trigger rule evaluation, and timetable calculations must live in pure modules with fast unit tests. Scheduler DB work should be thin glue around these pure helpers.

### DD-4: Signals Persist, Queries Do Not

Signals write to `harvest_signals`, enqueue a workflow task, and are recorded in event history when consumed. Queries must read current workflow state without mutating history. Cached-state lookup is preferred, replay-to-read is the fallback.

### DD-5: Saga Is A Workflow Helper, Not A New Runtime

The saga API should be a lightweight helper over existing workflow activity execution. It records compensation intent in workflow-local state and reuses normal activity execution/retry semantics instead of inventing a second orchestration engine.

---

## Task 1: Replace The Worker Dispatch Stub With Real Execution

**Files:**
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\worker.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\executor.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\queue.rs`
- Test: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\tests\integration_e2e.rs`

**Steps:**
1. Write a failing integration test proving a claimed workflow task advances through the real executor instead of being auto-completed with `null`.
2. Run that test and confirm it fails because the stub path marks the task complete without persisting the real workflow result.
3. Wire `dispatch_task()` to branch on task type and execute actual workflow/activity handling, including event loading, `run_workflow()`, and queue completion/failure.
4. Re-run the targeted integration test until it passes.

## Task 2: Add DAG Core Types And Topological Sort

**Files:**
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\dag.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\info.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\builder.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\lib.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\prelude.rs`

**Steps:**
1. Write unit tests for `DagBuilder` covering:
   - independent roots sharing level 0
   - downstream tasks landing in later execution levels
   - cycle detection returning an error
   - per-task trigger-rule and queue overrides
2. Run the new DAG unit tests and confirm they fail because the DAG module does not exist yet.
3. Implement:
   - `DagInfo`
   - `DagBuilder`
   - `DagTaskRef`
   - immutable `DagDefinition`
   - pure topological sort / cycle detection
4. Extend `HarvestBuilder` with `.dags(...)` registration and introspection helpers.
5. Re-run DAG unit tests until they pass.

## Task 3: Add `#[dag]` And `dags![]` Macros

**Files:**
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest-macros\src\lib.rs`
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest-macros\src\dag.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest-macros\src\collect.rs`
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\tests\macros_dag.rs`

**Steps:**
1. Write a failing proc-macro integration test that declares a sample `#[dag] fn daily_etl(...)` and asserts the generated `DagInfo` fields are collectable through `dags![daily_etl]`.
2. Run the new macro test and confirm it fails because `#[dag]`/`dags![]` do not exist.
3. Implement the macro so it emits `::autumn_harvest::DagInfo` using only `::autumn_harvest::` paths.
4. Re-run the macro test until it passes.

## Task 4: Build Scheduler Persistence And Tick Loop

**Files:**
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\scheduler.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\models.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\schema.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\lib.rs`
- Test: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\tests\scheduler_integration.rs`

**Steps:**
1. Write integration tests for:
   - schedule upsert into `harvest_schedules`
   - queued DAG run creation from a cron/manual timetable
   - activation honoring `max_active_runs`
2. Run them and confirm failure.
3. Implement scheduler store helpers plus a tick function that:
   - computes due runs
   - enqueues or activates runs
   - marks stuck runs failed
4. Keep `tick_once()` pure-ish at the orchestration level so it is testable without spawning a background loop.
5. Re-run scheduler integration tests until they pass.

## Task 5: Add Signal Delivery And Query Registry

**Files:**
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\context.rs`
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\signal.rs`
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\query.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\event.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\worker.rs`
- Test: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\tests\signals_queries.rs`

**Steps:**
1. Write unit and integration tests for:
   - `ctx.wait_for_signal()` replaying an already-recorded signal
   - live signal delivery through queued signal rows
   - `ctx.register_query()` returning current workflow state without recording new events
2. Run them and confirm failure.
3. Implement pending-signal storage helpers, signal dequeue/consume logic, query handler registration, and query execution against cached or replayed state.
4. Re-run signal/query tests until they pass.

## Task 6: Implement Saga Helper

**Files:**
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\saga.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\context.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\lib.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\prelude.rs`
- Test: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\tests\saga.rs`

**Steps:**
1. Write failing tests proving that if step 2 fails, compensation for step 1 runs in reverse order and the final error is preserved.
2. Run the tests and confirm failure.
3. Implement a minimal `Saga` helper that captures successful step outputs and runs compensation closures in reverse order.
4. Re-run saga tests until they pass.

## Task 7: Mount The Harvest Management API

**Files:**
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\api.rs`
- Create: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\ext.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\builder.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\src\lib.rs`
- Test: `C:\Users\markm\autumn\autumn-harvest\autumn-harvest\tests\management_api.rs`

**Steps:**
1. Write failing HTTP tests for the first management slice:
   - list workflow executions
   - fetch workflow history
   - send a signal
   - list DAG runs
   - inspect dead letters
2. Run them and confirm failure.
3. Implement Axum route handlers and a `HarvestExt` integration layer that mounts the router under the configured base path.
4. Re-run the HTTP tests until they pass.

## Task 8: Docs, Clippy, And Completion Gates

**Files:**
- Modify: `C:\Users\markm\autumn\autumn-harvest\CLAUDE.md`
- Modify: `C:\Users\markm\autumn\README.md`
- Modify: `C:\Users\markm\autumn\docs\autumn-workflow-architecture.md` if behavior diverged from the original design

**Steps:**
1. Update crate docs and examples for DAGs, signals, queries, saga, and API mounting.
2. Run the full Harvest test suite.
3. Run `cargo fmt`, `cargo clippy`, and targeted workspace tests.
4. Scan the touched area for `TODO`, `FIXME`, or stubs before claiming completion.

---

## Recommended Execution Order

1. Task 1 before everything else.
2. Tasks 2 and 3 next so DAG metadata exists before scheduler code.
3. Task 4 after DAG metadata is stable.
4. Tasks 5 and 6 can proceed in either order once the worker path is real.
5. Task 7 after the runtime primitives exist.
6. Task 8 last, with fresh verification only.

## Verification Commands

```bash
cd C:\Users\markm\autumn\autumn-harvest
cargo test -p autumn-harvest --no-default-features
cargo test -p autumn-harvest --features db
cargo test -p autumn-harvest-macros
cargo fmt --check
cargo clippy -p autumn-harvest --all-features -- -D warnings
cargo clippy -p autumn-harvest-macros -- -D warnings
```

Plan complete and saved to `docs/plans/2026-03-30-autumn-harvest-phase3.md`.

Two execution options:

1. Subagent-Driven (this session) - dispatch a fresh worker per task, review between tasks.
2. Parallel Session (separate) - open a dedicated execution session and work the plan checkpoint-by-checkpoint.

Recommended: Subagent-Driven in this session for Task 1 + Task 2 first, because the worker stub and DAG core are the current critical path.
