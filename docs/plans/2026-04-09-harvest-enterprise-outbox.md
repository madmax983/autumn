# Harvest Enterprise Outbox Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the reddit-clone-only Harvest workflow outbox with a framework-owned, lease-based, retry-aware outbox that works across embedded and split Harvest deployments.

**Architecture:** `autumn-web-harvest` owns the outbox schema, runtime config, relay loop, and dispatch helpers. The app database stores durable workflow-start intents; the Harvest database remains the sink for workflow executions. `HarvestExt` becomes responsible for starting and stopping the outbox relay and for applying Harvest storage migrations to the correct database in embedded vs split mode.

**Tech Stack:** Rust 2024, Diesel async/Postgres, tokio, Autumn `AppBuilder` startup hooks, `autumn-harvest` idempotent workflow start helper, Docker-backed integration tests via `testcontainers`.

---

### Task 1: Add the framework outbox model and configuration surface

**Files:**
- Create: `autumn-harvest/autumn-web-harvest/src/outbox.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/config.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/lib.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/prelude.rs`

**Step 1: Write the failing config tests**

Add unit tests in `autumn-harvest/autumn-web-harvest/src/config.rs` asserting:
- outbox defaults are enabled with sane batch/poll/lease/retry values
- env overrides such as `AUTUMN_HARVEST_OUTBOX__BATCH_SIZE` and `AUTUMN_HARVEST_OUTBOX__POLL_INTERVAL_MS` are honored
- invalid outbox values fail validation

**Step 2: Run the config tests to verify they fail**

Run: `cargo test -p autumn-web-harvest harvest_config_`

Expected: FAIL because the new outbox config fields do not exist yet.

**Step 3: Add minimal config and public API**

Implement:
- `HarvestOutboxConfig` on `HarvestRuntimeConfig`
- parse + validate support for:
  - `enabled`
  - `batch_size`
  - `poll_interval_ms`
  - `claim_ttl_ms`
  - `base_retry_delay_ms`
  - `max_retry_delay_ms`
- public re-exports from `lib.rs` and `prelude.rs`
- `WorkflowStartRequest` and outbox row/query types in `src/outbox.rs`

**Step 4: Re-run the config tests**

Run: `cargo test -p autumn-web-harvest harvest_config_`

Expected: PASS

### Task 2: Add RED tests for durable lease-based outbox delivery

**Files:**
- Create: `autumn-harvest/autumn-web-harvest/tests/outbox_integration.rs`
- Create: `autumn-harvest/autumn-web-harvest/migrations/20260409010000_harvest_workflow_outbox/up.sql`
- Create: `autumn-harvest/autumn-web-harvest/migrations/20260409010000_harvest_workflow_outbox/down.sql`

**Step 1: Write failing integration tests**

Cover these behaviors:
- a due outbox row in the app database is delivered into the Harvest database and marked delivered
- failed delivery records `last_error`, clears the claim, increments attempts, and schedules a retry in the future
- concurrent claim attempts from two drainers do not claim the same row twice

Use one Postgres container with two logical databases:
- app DB gets the framework outbox migration
- harvest DB gets Harvest core migrations

**Step 2: Run the integration tests to verify they fail**

Run: `cargo test -p autumn-web-harvest --test outbox_integration`

Expected: FAIL because the outbox module, migration, and relay logic are not implemented.

### Task 3: Implement the framework outbox runtime

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/outbox.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Modify: `autumn-harvest/autumn-web-harvest/src/state.rs`

**Step 1: Implement durable enqueue + claim + completion paths**

In `src/outbox.rs`, add:
- `enqueue_workflow_start_outbox(conn, request)`
- `flush_workflow_start_outbox(state)`
- `drain_workflow_start_outbox_once(state, limit)`
- internal SQL-backed claim helper using `FOR UPDATE SKIP LOCKED`
- per-row success/failure completion helpers
- exponential-ish retry scheduling capped by config

The outbox table should include at least:
- request payload columns: `workflow_name`, `workflow_id`, `queue_name`, `input`, `memo`, `search_attrs`
- delivery metadata: `delivery_attempts`, `last_error`, `delivered_execution_id`, `delivered_at`
- lease/retry metadata: `next_attempt_at`, `claimed_at`, `claimed_by`

**Step 2: Wire runtime ownership into `HarvestExt`**

Extend `HarvestRuntime` so startup:
- installs `HarvestDbPool` as before
- starts an outbox relay when an app DB is available and outbox is enabled
- stores the relay handle for coordinated shutdown

Shutdown must:
- cancel the relay
- await the task before returning

**Step 3: Re-run the outbox integration tests**

Run: `cargo test -p autumn-web-harvest --test outbox_integration`

Expected: PASS

### Task 4: Fix Harvest migration ownership for split mode

**Files:**
- Modify: `autumn-harvest/autumn-web-harvest/src/ext.rs`
- Modify: `autumn-harvest/autumn-web-harvest/tests/api_scheduler_integration.rs`

**Step 1: Add failing split-mode migration test**

Add a test proving:
- app DB receives the framework outbox migration
- split Harvest DB receives Harvest core tables
- app DB does not need Harvest system tables in split mode

**Step 2: Run that test to verify it fails**

Run: `cargo test -p autumn-web-harvest split_mode_`

Expected: FAIL because current migration ownership is still bound to the app builder.

**Step 3: Move Harvest storage migration responsibility under `HarvestExt` startup**

Implement startup migration helpers so:
- outbox migration targets the app DB
- Harvest core migrations target the resolved Harvest DB
- embedded mode migrates both roles against the same logical DB
- split mode migrates app outbox and Harvest storage separately

Preserve existing dev/prod semantics:
- dev applies pending migrations automatically
- non-dev logs pending state instead of mutating

**Step 4: Re-run split-mode migration tests**

Run: `cargo test -p autumn-web-harvest split_mode_`

Expected: PASS

### Task 5: Migrate reddit-clone onto the framework outbox

**Files:**
- Delete: `examples/reddit-clone/src/outbox.rs`
- Modify: `examples/reddit-clone/src/workflows.rs`
- Modify: `examples/reddit-clone/src/routes/auth.rs`
- Modify: `examples/reddit-clone/src/routes/posts.rs`
- Modify: `examples/reddit-clone/src/main.rs`
- Modify: `examples/reddit-clone/src/models.rs`
- Modify: `examples/reddit-clone/src/schema.rs`
- Modify: `examples/reddit-clone/migrations/00000000000000_create_reddit/up.sql`
- Modify: `examples/reddit-clone/migrations/00000000000000_create_reddit/down.sql`

**Step 1: Write failing reddit-clone tests**

Add or update tests so the example proves:
- registration writes a framework outbox row transactionally
- post submission writes a framework outbox row transactionally
- the example no longer owns a bespoke outbox schema/module

**Step 2: Run the example tests to verify they fail**

Run: `cargo test -p reddit-clone workflows`

Expected: FAIL because the example still references its local outbox module and schema.

**Step 3: Replace local outbox code with framework calls**

Use `autumn_web_harvest` helpers from route transactions and remove:
- local relay startup/shutdown code
- local outbox schema/models
- duplicated dispatch request type

Keep reddit-clone responsible only for building the workflow-start payloads.

**Step 4: Re-run reddit-clone tests**

Run: `cargo test -p reddit-clone workflows`

Expected: PASS

### Task 6: Verify the whole slice and document the new story

**Files:**
- Modify: `examples/reddit-clone/README.md`
- Modify: `docs/adr/TD-008-harvest-topology-progression.md`
- Modify: `docs/plans/2026-04-09-harvest-topology-roadmap.md`

**Step 1: Update docs**

Document:
- embedded vs split outbox behavior
- that app writes + outbox row are atomic in the app DB
- that delivery into Harvest is at-least-once and relies on idempotent workflow start
- that split mode now owns its own Harvest migrations

**Step 2: Run final verification**

Run:
- `cargo fmt --all`
- `cargo test -p autumn-web-harvest --lib`
- `cargo test -p autumn-web-harvest --test api_scheduler_integration`
- `cargo test -p autumn-web-harvest --test outbox_integration`
- `cargo test -p reddit-clone workflows`
- `cargo test -p reddit-clone --no-run -j 1`

Expected: PASS

**Step 3: Review dirty-tree scope**

Confirm the diff removes the example-local outbox implementation instead of leaving duplicate systems behind.
