# Reddit Clone Harvest Runner Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a first-party Harvest runner binary to `reddit-clone` so the example has a real external-runtime escape hatch instead of only a library-level API.

**Architecture:** Split `reddit-clone` into a small reusable library seam that owns route/task/workflow registration, keep `src/main.rs` as the embedded happy-path web entry point, and add a second binary that loads Autumn + Harvest config, resolves app and Harvest pools, and starts `HarvestRunner` with the same workflow/activity registration set.

**Tech Stack:** Rust 2024, `autumn-web`, `autumn-web-harvest`, Tokio, Diesel Async, Postgres.

---

### Task 1: Write The Red Tests

**Files:**
- Create: `examples/reddit-clone/src/harvest_runtime.rs`
- Create: `examples/reddit-clone/src/bin/harvest-runner.rs`

**Step 1: Write failing tests**

Add tests for:
- a shared Harvest builder helper reusing the registered workflows and activities
- runner config helpers resolving the app DB and Harvest DB URLs correctly
- runner startup rejecting configs with neither worker nor scheduler ownership enabled

**Step 2: Run tests to verify they fail**

Run targeted reddit-clone tests for the new helpers.

### Task 2: Add The Reusable Library Seam And Runner Binary

**Files:**
- Create: `examples/reddit-clone/src/lib.rs`
- Modify: `examples/reddit-clone/src/main.rs`
- Create: `examples/reddit-clone/src/harvest_runtime.rs`
- Create: `examples/reddit-clone/src/bin/harvest-runner.rs`
- Modify: `examples/reddit-clone/Cargo.toml`

**Step 1: Write minimal implementation**

- Move module ownership to `lib.rs`
- Expose a shared Harvest builder/config helper
- Keep the web main binary thin
- Add a runner binary that:
  - loads config
  - builds app + Harvest pools
  - constructs a detached `AppState`
  - starts `HarvestRunner`
  - waits for Ctrl+C and shuts down cleanly

**Step 2: Run tests to verify they pass**

Run the targeted helper tests and compile both binaries.

### Task 3: Document The Example Escape Hatch

**Files:**
- Modify: `examples/reddit-clone/README.md`
- Create: `examples/reddit-clone/autumn-external-web.toml`
- Create: `examples/reddit-clone/autumn-external-runner.toml`

**Step 1: Update docs/config**

- Add explicit example profiles for web-side external mode and runner-side ownership
- Document exact commands for running both processes
- Call out any current process-local caveats honestly

**Step 2: Verify**

Run:
- `cargo fmt --all`
- `cargo test -p reddit-clone --lib`
- `cargo test -p reddit-clone --bin reddit-clone --no-run -j 1`
- `cargo test -p reddit-clone --bin reddit-clone-harvest-runner --no-run -j 1`
