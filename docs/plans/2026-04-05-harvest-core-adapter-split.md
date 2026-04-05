# Harvest Core/Adapter Split Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split Autumn-specific integration out of Harvest so the workflow engine can live as a framework-agnostic core crate.

**Architecture:** Keep `autumn-harvest` as the engine core crate and add a thin `autumn-harvest-autumn` adapter crate for `HarvestExt`, the HTTP management API, and app lifecycle wiring. Preserve existing runtime behavior by moving the Autumn glue rather than rewriting the engine.

**Tech Stack:** Rust 2024, Cargo workspaces, Diesel, Tokio, Axum, Autumn `AppBuilder`, autumn-harvest macros.

---

### Task 1: Define the new crate boundary

**Files:**
- Modify: `autumn-harvest/Cargo.toml`
- Modify: `autumn-harvest/autumn-harvest/Cargo.toml`
- Create: `autumn-harvest/autumn-harvest-autumn/Cargo.toml`

**Step 1: Write the failing test**

Use the existing compile surface as the contract: the new adapter crate must compile with the copied API/ext files and the Reddit example must still type-check once imports move.

**Step 2: Run test to verify it fails**

Run: `cargo check --workspace` from `autumn-harvest`
Expected: FAIL until the new adapter crate exists and imports are corrected.

**Step 3: Write minimal implementation**

- Add `autumn-harvest-autumn` to the nested Harvest workspace.
- Remove `autumn-web` from the core crate.
- Add the adapter crate depending on `autumn-harvest` and `autumn-web`.

**Step 4: Run test to verify it passes**

Run: `cargo check --workspace`
Expected: adapter and core both compile.

### Task 2: Move Autumn glue into the adapter crate

**Files:**
- Create: `autumn-harvest/autumn-harvest-autumn/src/lib.rs`
- Create: `autumn-harvest/autumn-harvest-autumn/src/prelude.rs`
- Create: `autumn-harvest/autumn-harvest-autumn/src/api.rs`
- Create: `autumn-harvest/autumn-harvest-autumn/src/ext.rs`
- Delete: `autumn-harvest/autumn-harvest/src/api.rs`
- Delete: `autumn-harvest/autumn-harvest/src/ext.rs`
- Modify: `autumn-harvest/autumn-harvest/src/lib.rs`
- Modify: `autumn-harvest/autumn-harvest/src/prelude.rs`

**Step 1: Write the failing test**

Use the moved `api_scheduler_integration.rs` plus existing `ext.rs` unit tests as the specification.

**Step 2: Run test to verify it fails**

Run: `cargo test -p autumn-harvest-autumn --test api_scheduler_integration --no-run`
Expected: FAIL until imports and exports are corrected.

**Step 3: Write minimal implementation**

- Move `HarvestExt`, router state, and HTTP handlers into the adapter crate.
- Re-export core prelude items plus adapter-specific items from the adapter prelude.
- Remove Autumn-specific exports from the core prelude/lib.

**Step 4: Run test to verify it passes**

Run: `cargo test -p autumn-harvest-autumn --test api_scheduler_integration --no-run`
Expected: PASS.

### Task 3: Remove Autumn helper usage from core

**Files:**
- Modify: `autumn-harvest/autumn-harvest/src/lib.rs`

**Step 1: Write the failing test**

Add or preserve unit tests for duration parsing.

**Step 2: Run test to verify it fails**

Run: `cargo test -p autumn-harvest task_duration --lib`
Expected: FAIL until the parser is local to Harvest core.

**Step 3: Write minimal implementation**

- Replace `autumn_web::task::parse_duration` with a local parser in core.

**Step 4: Run test to verify it passes**

Run: `cargo test -p autumn-harvest task_duration --lib`
Expected: PASS.

### Task 4: Update consumers and CI

**Files:**
- Modify: `examples/reddit-clone/Cargo.toml`
- Modify: `examples/reddit-clone/src/main.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Modify: `autumn-harvest/CLAUDE.md`

**Step 1: Write the failing test**

Use `cargo check --workspace` and targeted clippy/test runs as the consumer contract.

**Step 2: Run test to verify it fails**

Run: `cargo check --workspace`
Expected: FAIL until the example imports the adapter crate correctly.

**Step 3: Write minimal implementation**

- Add the adapter dependency to the real consumer.
- Import `HarvestExt` from the adapter crate.
- Teach CI to lint/test the new adapter crate.
- Update docs to reflect the new boundary.

**Step 4: Run test to verify it passes**

Run: `cargo check --workspace`
Expected: PASS.

### Task 5: Verify the split end to end

**Files:**
- Modify as needed based on failures discovered in verification

**Step 1: Run verification**

Run:
- `cargo fmt --all`
- `cargo clippy -p autumn-harvest --all-features --tests -- -D warnings`
- `cargo test -p autumn-harvest --no-default-features`
- `cargo test -p autumn-harvest --all-features --lib`
- `cargo clippy -p autumn-harvest-autumn --all-targets -- -D warnings`
- `cargo test -p autumn-harvest-autumn --lib`
- `cargo test -p autumn-harvest-autumn --test api_scheduler_integration --no-run`
- `cargo test -p reddit-clone`
- `cargo check --workspace`

**Step 2: Fix fallout**

Patch any import/export, feature, or doc drift until all relevant checks are green.
