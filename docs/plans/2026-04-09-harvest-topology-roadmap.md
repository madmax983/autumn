# Autumn Harvest Topology Roadmap

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let Autumn Harvest grow from "same logical DB, embedded mode" to
"separate logical DB on the same Postgres instance" to "separate Postgres
cluster" without changing the workflow/activity programming model.

**Architecture:** Preserve the current macro and builder surface, but separate
Harvest's system-storage role from the application's business-storage role. In
`embedded` mode both roles may resolve to the same DSN; in `split` and
`external` modes they diverge by configuration. Runtime ownership should also
progress: embedded defaults to in-process worker/scheduler, while external mode
must support dedicated Harvest runner processes.

**Tech Stack:** Rust 2024, Autumn `AppBuilder`, autumn-harvest core,
autumn-web-harvest adapter, Diesel, diesel-async, deadpool, Postgres.

**Depends on:** The existing core/adapter split, current embedded integration,
and the existing `reddit-clone` and `bookmarks-distributed` examples.

---

## Design Decisions Baked Into This Plan

### DD-1: Two Database Roles, Even In Embedded Mode

Treat application storage and Harvest storage as distinct roles from day one.
`embedded` mode is "same DSN, different role," not "one pool for everything."

### DD-2: Three Named Topology Modes

Expose the progression directly:

- `embedded`: same logical DB, in-process worker/scheduler
- `split`: separate logical DB on same Postgres instance
- `external`: separate Postgres cluster, with dedicated runtime support

The names matter because they give users a roadmap instead of a bag of flags.

### DD-3: Split/External Require Honest Delivery Semantics

Once Harvest no longer shares the same database as app writes, the framework
must stop pretending those operations are atomically coupled. The supported seam
becomes idempotent workflow start/signal plus outbox-driven publication when
durable coupling is required.

Current status:

- workflow start is idempotent on `(workflow_name, workflow_id)`
- `autumn-web-harvest` owns a durable workflow-start outbox with lease/retry semantics
- `reddit-clone` now uses the framework outbox instead of an app-local relay

### DD-4: Example Strategy Mirrors `bookmarks` vs `bookmarks-distributed`

Keep `examples/reddit-clone` as the happy-path embedded story. Add a future
distributed sibling only after the topology seams are real, so the docs show
"how you grow up" rather than "how to suffer early."

---

## Task 1: Add Explicit Harvest Topology Configuration

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Create: `autumn-harvest/autumn-web-harvest/src/config.rs`
- Modify: `autumn/` configuration integration as needed
- Modify: docs and example config files

**Step 1: Write the failing test**

Add unit tests for config resolution covering:

- `embedded` defaults Harvest DB URL to the app DB URL
- `split` requires `harvest.database.url`
- `external` requires `harvest.database.url`
- `external` can disable in-process worker/scheduler by configuration

**Step 2: Run test to verify it fails**

Run targeted adapter/config tests.

**Step 3: Write minimal implementation**

Introduce a typed Harvest config surface, for example:

```toml
[harvest]
mode = "embedded"
worker_enabled = true
scheduler_enabled = true

[harvest.database]
url = "postgres://..."
```

Resolve config so that embedded mode remains zero-friction.

**Step 4: Run test to verify it passes**

All new topology-resolution tests should pass.

## Task 2: Separate Harvest Storage From App Storage In Runtime State

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Modify: `autumn-harvest/autumn-harvest/src/worker.rs` only if required for pool typing
- Modify: example activities that currently depend on the shared `DbPool`

**Step 1: Write the failing test**

Use adapter unit tests plus a small integration test proving that:

- Harvest runtime can boot with a Harvest DB pool resolved independently
- activities can still access application storage through an explicit app-state seam

**Step 2: Run test to verify it fails**

The current implementation should fail because startup assumes `AppState.pool()`
is the Harvest store.

**Step 3: Write minimal implementation**

- Define distinct injected state roles such as `HarvestDbPool` and `AppDbPool`
  (or equivalent typed wrappers).
- Keep the Harvest core worker bound to Harvest system storage.
- Update `reddit-clone` activities to request the application DB role
  explicitly instead of reusing Harvest's pool by accident.

**Step 4: Run test to verify it passes**

Embedded mode still works, but the seam is now explicit.

## Task 3: Split Migration Ownership And Tooling

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Create or modify CLI/migrator entry points
- Modify: example READMEs and deployment docs

**Step 1: Write the failing test**

Add coverage proving that Harvest migrations target the Harvest DB role instead
of whichever app DB happened to be configured first.

**Step 2: Run test to verify it fails**

Current behavior should show that Harvest migration ownership is still tied to
the app database startup path.

**Step 3: Write minimal implementation**

Provide an explicit migration story:

- embedded: app startup may still auto-apply Harvest migrations in dev
- split/external: explicit Harvest migrator path should exist

The preferred long-term UX is either a dedicated Harvest migrator binary or a
CLI flow such as `autumn harvest migrate`.

**Step 4: Run test to verify it passes**

Migration targeting should be deterministic in all three modes.

## Task 4: Make App-To-Harvest Delivery Idempotent

**Files:**
- Modify: workflow start/signal helpers in app examples
- Create: outbox helper abstractions if needed
- Modify: Harvest API/client surface as needed

**Step 1: Write the failing test**

Add tests that model retries and duplicate publication:

- starting the same workflow twice with the same idempotency key is safe
- delivering the same signal twice is safe or deterministically rejected

**Step 2: Run test to verify it fails**

The current embedded-only assumptions should expose gaps once the DB roles are
split.

**Step 3: Write minimal implementation**

- Standardize idempotency keys for workflow start/signal calls
- Add an outbox-backed publication path for cases where app writes and Harvest
  dispatch must survive process or network failure between stores

**Step 4: Run test to verify it passes**

The framework now has an honest story once users leave embedded mode.

## Task 5: Add External Runner Mode

Current status:

- `autumn-web-harvest` now supports `external` mode without forcing local
  worker/scheduler ownership.
- Runtime ownership is decided by `worker_enabled` and `scheduler_enabled`.
- The reusable standalone entry point is `HarvestRunner`.

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Create: dedicated Harvest runner binary or reusable startup entry point
- Modify: runtime docs and deployment examples

**Step 1: Write the failing test**

Prove that a web app can mount Harvest APIs and register workflows without
starting an in-process worker/scheduler when running in `external` mode.

**Step 2: Run test to verify it fails**

Current startup should still assume in-process ownership.

**Step 3: Write minimal implementation**

- Allow worker and scheduler startup to be independently disabled
- Provide a dedicated runtime entry point for the external Harvest process
- Keep management/query APIs usable from the web side

**Step 4: Run test to verify it passes**

External mode should be a real deployment option, not just a config placeholder.

## Task 6: Tell The Growth Story In Docs And Examples

**Files:**
- Modify: `README.md`
- Modify: Harvest docs
- Keep: `examples/reddit-clone` as embedded default
- Create later: `examples/reddit-clone-distributed` or equivalent

**Step 1: Write the failing test**

Use docs/example review as the contract:

- embedded example remains dead simple
- split/external path is documented without hand-wavy caveats
- the growth story mirrors `bookmarks` to `bookmarks-distributed`

**Step 2: Run test to verify it fails**

Current docs do not yet offer a coherent topology progression story.

**Step 3: Write minimal implementation**

- Document the three modes with exact config and migration steps
- Explain when to stay embedded and when to split
- Add a distributed example only after the runtime seam is stable enough to
  demonstrate without framework-internal caveats

**Step 4: Run test to verify it passes**

A user should be able to understand:

1. how to start in embedded mode
2. when to move to split mode
3. how to graduate to external mode without rewriting workflows

---

## Verification

Before calling this roadmap implemented, verify all of the following:

- Embedded example (`reddit-clone`) still boots and runs with the zero-config story
- Split mode can point Harvest at a different logical database on the same
  Postgres instance
- External mode can run with the web app and Harvest runtime in separate
  processes
- Workflow start/signal semantics are idempotent across retries
- Harvest migrations can be applied deterministically for every topology
- Documentation shows the growth path explicitly, not as an afterthought

## Suggested Rollout Order

1. Config surface and storage-role separation
2. Migration ownership and tooling
3. Idempotent delivery/outbox seam
4. External runner mode
5. Distributed example and polished documentation
