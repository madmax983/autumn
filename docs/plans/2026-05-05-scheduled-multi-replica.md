# Scheduled Multi-Replica Safety Implementation Plan

**Goal:** Make `#[scheduled]` tasks optionally fleet-coordinated so a multi-replica deployment can run one task invocation per scheduled tick.

**Architecture:** Keep current per-process behavior as the default `scheduler.backend = "in_process"`. Add scheduler coordination metadata to `TaskInfo`, a config-driven coordinator abstraction, and a Postgres advisory-lock backend that reuses the existing `Db` pool. Extend `/actuator/tasks` with backend, replica, leader, and last-fired visibility.

**Tech Stack:** Rust 2024, Tokio, Diesel async Postgres pool, `tokio-cron-scheduler`, Axum actuator JSON, proc macros, TDD red/green/refactor.

---

### Task 1: Runtime Coordination Types

**Files:**
- Modify: `autumn/src/task.rs`
- Create: `autumn/src/scheduler.rs`
- Modify: `autumn/src/lib.rs`

**Steps:**
1. Write failing unit tests for default fleet coordination metadata, per-replica opt-out metadata, deterministic tick keys, and in-process coordinator acquisition.
2. Run `cargo test -p autumn-web task:: scheduler::`.
3. Implement `TaskCoordination`, `SchedulerCoordinator`, in-process coordinator, tick-key helpers, and exported module wiring.
4. Re-run the same targeted tests.

### Task 2: Config Surface

**Files:**
- Modify: `autumn/src/config.rs`
- Test: `autumn/src/config.rs`

**Steps:**
1. Write failing tests for `[scheduler] backend = "postgres"`, default `in_process`, `lease_ttl_secs`, `replica_id`, and env overrides.
2. Run targeted config tests.
3. Implement `SchedulerConfig`, `SchedulerBackend`, defaults, deserialization, validation, and env overrides.
4. Re-run targeted config tests.

### Task 3: Macro Metadata

**Files:**
- Modify: `autumn-macros/src/scheduled.rs`
- Test: `autumn/tests/compile-pass/task_basic.rs` or a new compile-pass file

**Steps:**
1. Add a failing compile-pass test proving `#[scheduled(..., coordination = "per_replica")]` compiles and `tasks![]` returns metadata.
2. Run the compile-pass test.
3. Parse the optional coordination attribute and populate `TaskInfo`.
4. Re-run the compile-pass test.

### Task 4: Scheduler Execution And Postgres Locks

**Files:**
- Modify: `autumn/src/app.rs`
- Modify: `autumn/src/scheduler.rs`
- Test: `autumn/src/app.rs`, `autumn/src/scheduler.rs`

**Steps:**
1. Write failing tests showing a skipped lease does not invoke the handler and a per-replica task bypasses fleet locking.
2. Write Postgres advisory-lock unit tests around deterministic lock-key hashing; integration with a real pool remains best-effort if local Docker/Testcontainers is available.
3. Implement coordinator construction from config, acquisition before each fixed-delay or cron run, Postgres advisory lock acquire/release, and task-registry leader updates.
4. Re-run targeted scheduler/app tests.

### Task 5: Actuator And Docs

**Files:**
- Modify: `autumn/src/actuator.rs`
- Modify: `docs/guide/scheduled-multi-replica.md`
- Modify: `README.md`

**Steps:**
1. Write failing actuator tests for leader/replica/last-fired fields in `/actuator/tasks`.
2. Extend `TaskStatus` and `TaskRegistry` to expose coordination fields.
3. Add production guidance and README Local-Safe vs Production-Safe coverage.
4. Run formatting, targeted tests, and affected-area stub scan.

---

### Verification

- `cargo fmt`
- `cargo clippy -p autumn-web --lib -- -D warnings`
- `cargo test -p autumn-web --lib`
- `cargo test -p autumn-web --test scheduled_coordination`
- `cargo test -p autumn-web --test compile_fail`
- Affected-area scan for unfinished-work markers
