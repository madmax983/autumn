# Harvest External Runner Mode Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `harvest.mode = "external"` a real deployment option where the web app can keep Harvest APIs and outbox delivery without owning the worker/scheduler, while a separate process can run the Harvest runtime against the same Harvest store.

**Architecture:** Split Harvest startup into two layers: runtime preparation and runtime ownership. `HarvestExt` should prepare registrations, storage pools, migrations, API state, and optional local runtime ownership based on `worker_enabled` / `scheduler_enabled`. A new reusable standalone runner surface should consume `HarvestBuilder::build()` plus explicit pool/config inputs so the external process reuses the same runtime bootstrap instead of inventing a second registration model.

**Tech Stack:** Rust 2024, `autumn-web`, `autumn-web-harvest`, `autumn-harvest`, Tokio, Axum, Diesel Async, deadpool, Postgres testcontainers.

---

### Task 1: Replace The External Placeholder With Red Tests

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Modify: `autumn-harvest/autumn-web-harvest/tests/api_scheduler_integration.rs`

**Step 1: Write the failing test**

Add coverage for:
- external mode no longer being rejected when local worker/scheduler ownership is disabled
- web-side runtime preparation installing an API runtime with offline scheduler state and no worker id when local ownership is disabled
- standalone runner bootstrap being able to own worker/scheduler against the Harvest store and drive a started workflow to completion

**Step 2: Run test to verify it fails**

Run:
- `cargo test -p autumn-web-harvest external_mode`
- `cargo test -p autumn-web-harvest --test api_scheduler_integration external_runner`

Expected: failure because `ext.rs` still rejects external mode and no standalone runner surface exists.

### Task 2: Refactor Runtime Ownership And Add The Standalone Runner

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/api.rs`
- Create: `autumn-harvest/autumn-web-harvest/src/runner.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/lib.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/prelude.rs`

**Step 1: Write minimal implementation**

- Introduce a reusable runtime-preparation path that turns `BuiltHarvest` plus injected state into:
  - `HandlerRegistry`
  - compiled DAG catalog
  - worker runtime config
  - API runtime snapshot
- Make API runtime model optional local ownership honestly, especially `worker_id`
- Let `HarvestExt` conditionally start worker and scheduler according to config instead of hard-rejecting the toggles
- Add a standalone runner type/function that starts local ownership against an explicit Harvest pool and can be cleanly shut down

**Step 2: Run tests to verify they pass**

Run the new targeted tests plus the adapter unit tests.

### Task 3: Document The External Deployment Story And Verify It End-To-End

**Files:**
- Modify: `docs/adr/TD-008-harvest-topology-progression.md`
- Modify: `docs/plans/2026-04-09-harvest-topology-roadmap.md`
- Modify: `examples/reddit-clone/README.md`

**Step 1: Update docs**

- Show the exact `external` config with `worker_enabled = false` and `scheduler_enabled = false` in the web app
- Show the separate runner bootstrap using the new standalone API
- Clarify that external mode means separate Harvest storage, while runtime ownership is decided by the toggles

**Step 2: Run verification**

Run:
- `cargo fmt --all`
- `cargo test -p autumn-web-harvest --lib`
- `cargo test -p autumn-web-harvest --test api_scheduler_integration`
- `cargo test -p autumn-web-harvest --test outbox_integration`
- `cargo test -p reddit-clone --no-run -j 1`

Expected: all pass, with the container-backed tests proving the web app and dedicated runner can share the Harvest store without in-process worker ownership.
