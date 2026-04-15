# Harvest Topology Config Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an explicit Harvest topology configuration surface that supports `embedded`, `split`, and `external` modes without breaking the current zero-config embedded experience.

**Architecture:** Keep the first slice narrowly scoped to configuration resolution in `autumn-web-harvest`. The adapter will load Harvest-specific topology settings from the same `autumn.toml` / `autumn-{profile}.toml` / `AUTUMN_*` layering model as Autumn itself, but without yet rewriting the full runtime ownership boundary. This lets us prove the topology modes, validation, and fallback behavior before we separate pools and startup ownership.

**Tech Stack:** Rust 2024, `autumn-web-harvest`, `autumn` config conventions, `serde`, `toml`, unit tests in adapter crate.

---

### Task 1: Introduce Typed Harvest Topology Config

**Files:**
- Create: `autumn-harvest/autumn-web-harvest/src/config.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/lib.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/prelude.rs`

**Step 1: Write the failing test**

Add unit tests in `config.rs` covering:

- `embedded` mode defaults to `HarvestMode::Embedded`
- `embedded` with no `harvest.database.url` resolves to "reuse app database URL"
- `split` without `harvest.database.url` fails validation
- `external` without `harvest.database.url` fails validation
- `external` allows `worker_enabled = false` and `scheduler_enabled = false`
- env overrides like `AUTUMN_HARVEST__MODE=external` and `AUTUMN_HARVEST_DATABASE__URL=...` win over TOML

**Step 2: Run test to verify it fails**

Run: `cargo test -p autumn-web-harvest harvest_config --lib`
Expected: FAIL because `config.rs` and the new types do not exist yet.

**Step 3: Write minimal implementation**

Create:

- `HarvestMode` enum with `Embedded`, `Split`, `External`
- `HarvestRuntimeConfig` struct with:
  - `mode: HarvestMode`
  - `worker_enabled: bool`
  - `scheduler_enabled: bool`
  - `database: HarvestDatabaseConfig`
- `HarvestDatabaseConfig` with `url: Option<String>`

Implement:

- TOML loading from `autumn.toml` and `autumn-{profile}.toml`
- env overrides:
  - `AUTUMN_HARVEST__MODE`
  - `AUTUMN_HARVEST__WORKER_ENABLED`
  - `AUTUMN_HARVEST__SCHEDULER_ENABLED`
  - `AUTUMN_HARVEST_DATABASE__URL`
- validation rules:
  - embedded: DB URL optional
  - split/external: DB URL required

Keep this self-contained inside the adapter crate for now.

**Step 4: Run test to verify it passes**

Run: `cargo test -p autumn-web-harvest harvest_config --lib`
Expected: PASS.

### Task 2: Thread Harvest Config Into Adapter Startup

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Test: `autumn-harvest/autumn-web-harvest/src/ext.rs`

**Step 1: Write the failing test**

Add adapter tests proving:

- startup resolves a Harvest config successfully in embedded mode without requiring new user config
- `split`/`external` config validation errors surface as startup errors instead of silent fallback

Do not test separate pools yet. Only test config resolution and validation plumbing.

**Step 2: Run test to verify it fails**

Run: `cargo test -p autumn-web-harvest harvest_ext --lib`
Expected: FAIL because `ext.rs` does not know about the new Harvest config surface.

**Step 3: Write minimal implementation**

In `ext.rs`:

- load `HarvestRuntimeConfig` during startup
- keep current embedded behavior as the fallback runtime shape
- for now, reject `split` and `external` with explicit `AutumnError::service_unavailable_msg(...)` if the mode is configured but runtime support is not yet implemented

This preserves forward progress without pretending the topology is already fully supported.

**Step 4: Run test to verify it passes**

Run: `cargo test -p autumn-web-harvest harvest_ext --lib`
Expected: PASS.

### Task 3: Document The Config Surface Without Overpromising

**Files:**
- Modify: `docs/adr/TD-008-harvest-topology-progression.md`
- Modify: `docs/plans/2026-04-09-harvest-topology-roadmap.md`
- Modify: `autumn-harvest/CLAUDE.md`

**Step 1: Write the failing test**

Use doc review as the contract:

- docs must say Task 1 introduces topology config and validation
- docs must not claim split/external runtime support exists yet

**Step 2: Run review to verify current docs are stale**

Read the new docs and check whether they need a note about incremental support status.

**Step 3: Write minimal implementation**

Update docs to say:

- config surface lands first
- embedded remains the default supported mode
- split/external config is groundwork until later roadmap tasks land

**Step 4: Run review to verify it is accurate**

Re-read the edited docs and ensure they match the actual implementation status.

### Task 4: Verify The First Slice

**Files:**
- Modify as needed based on fallout

**Step 1: Run verification**

Run:

- `cargo fmt --all`
- `cargo test -p autumn-web-harvest harvest_config --lib`
- `cargo test -p autumn-web-harvest harvest_ext --lib`
- `cargo test -p reddit-clone --no-run`

**Step 2: Fix fallout**

Patch any adapter import/export drift or startup-validation behavior until the
checks above pass.

**Step 3: Commit**

```bash
git add docs/adr/TD-008-harvest-topology-progression.md \
        docs/plans/2026-04-09-harvest-topology-roadmap.md \
        docs/plans/2026-04-09-harvest-topology-task1-config.md \
        autumn-harvest/autumn-web-harvest/src/config.rs \
        autumn-harvest/autumn-web-harvest/src/ext.rs \
        autumn-harvest/autumn-web-harvest/src/lib.rs \
        autumn-harvest/autumn-web-harvest/src/prelude.rs \
        autumn-harvest/CLAUDE.md
git commit -m "feat: add Harvest topology config surface"
```
