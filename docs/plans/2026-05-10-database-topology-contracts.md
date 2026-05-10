# Database Topology Contracts Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make primary/replica database topology a first-class Autumn contract instead of example-local glue.

**Architecture:** Extend the core `[database]` config with canonical primary and replica roles while keeping `database.url` as the single-primary compatibility path. Build a primary pool plus optional replica pool in framework state, keep `Db` on the primary role, add a read-role extractor, and teach CLI/docs/templates that migrations target only the primary role.

**Tech Stack:** Rust 2024, Autumn Web, diesel-async/deadpool, autumn-cli, TOML config, Markdown docs.

---

### Task 1: Core Config Contract

**Files:**
- Modify: `autumn/src/config.rs`
- Test: `autumn/src/config.rs`

**Step 1: Write failing tests**

Add tests for:
- `database.primary_url` and `database.replica_url` deserialize from TOML.
- `database.url` remains a valid single-primary compatibility path.
- `database.replica_url` without `url` or `primary_url` fails validation.
- env vars `AUTUMN_DATABASE__PRIMARY_URL`, `AUTUMN_DATABASE__REPLICA_URL`, and replica fallback override config.

**Step 2: Run tests to verify RED**

Run: `cargo test -p autumn-web database_topology --features db`

Expected: compile/test failure because fields and helpers do not exist.

**Step 3: Implement minimal config**

Add role fields, a `ReplicaFallback` enum, URL/pool helper methods, validation, defaults, and env overrides.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p autumn-web database_topology --features db`

Expected: PASS.

### Task 2: Runtime Pool Contract

**Files:**
- Modify: `autumn/src/db.rs`
- Modify: `autumn/src/state.rs`
- Modify: `autumn/src/app.rs`
- Test: `autumn/src/db.rs`, `autumn/src/state.rs`

**Step 1: Write failing tests**

Add tests for:
- topology creation builds primary and replica pools with role-specific pool sizes.
- single-url apps still create only a primary pool.
- `ReadDb` falls back to primary when no replica exists.

**Step 2: Run tests to verify RED**

Run: `cargo test -p autumn-web database_topology --features db`

Expected: failure because topology/replica APIs do not exist.

**Step 3: Implement minimal runtime**

Add primary/replica pool creation, AppState storage/accessors, `DbState::read_pool`, and a `ReadDb` extractor.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p autumn-web database_topology --features db`

Expected: PASS.

### Task 3: CLI Primary-Role Migrations and Doctor Checks

**Files:**
- Modify: `autumn-cli/src/migrate.rs`
- Modify: `autumn-cli/src/doctor.rs`
- Test: `autumn-cli/src/migrate.rs`, `autumn-cli/src/doctor.rs`

**Step 1: Write failing tests**

Add tests proving:
- `autumn migrate` resolves `AUTUMN_DATABASE__PRIMARY_URL` before legacy URLs and never chooses replica.
- doctor topology checks fail replica-without-primary and prod replica plus startup migrations.
- stale replica migration versions fail when fallback is `fail_readiness`.
- diagnostics include role names and omit credentials.

**Step 2: Run tests to verify RED**

Run: `cargo test -p autumn-cli database_topology`

Expected: failure because topology parsing/checking does not exist.

**Step 3: Implement minimal CLI behavior**

Resolve primary role in migrate, add pure doctor topology helpers, add role-aware connectivity checks, and wire checks into `run`.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p autumn-cli database_topology`

Expected: PASS.

### Task 4: Docs, Templates, and Certified Example

**Files:**
- Modify: `autumn-cli/src/templates/autumn.toml.tmpl`
- Modify: `autumn-cli/src/templates/release/*.tmpl`
- Modify: `autumn-cli/src/release.rs`
- Modify: `examples/bookmarks-distributed/*`
- Modify: `README.md`, `docs/guide/cloud-native.md`, `docs/guide/deployment.md`, `docs/guide/tutorial/03-database.md`, `docs/guide/tutorial/10-configuration.md`, `docs/guide/what-happens-when.md`

**Step 1: Write failing tests**

Add/adjust release template tests that reject production auto-migrate defaults and require primary-role comments.

**Step 2: Run tests to verify RED**

Run: `cargo test -p autumn-cli release`

Expected: failure before template edits.

**Step 3: Implement docs/templates**

Replace startup migration guidance with one-shot migrator guidance, document supported topologies, and migrate the distributed example to canonical `[database]` fields.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p autumn-cli release`

Expected: PASS.

### Task 5: Verification

Run:
- `cargo fmt`
- `cargo test -p autumn-web --features db database_topology`
- `cargo test -p autumn-cli database_topology`
- `cargo test -p autumn-cli release`
- affected docs/template grep for stale `auto_migrate_in_production = true`
- affected-area TODO/FIXME scan

Expected: all targeted tests pass; any remaining failures are reported with exact scope.
