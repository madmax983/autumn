# Reddit Clone Harvest Outbox Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make reddit-clone’s app-to-Harvest publication durable when the app database and Harvest database are separate stores.

**Architecture:** Persist workflow-start requests into an app-db outbox inside the same transaction as the user/post write, then drain that outbox into Harvest using Harvest’s idempotent start helper. Keep the relay intentionally simple: immediate best-effort drain after commits plus a startup background poller that retries any pending rows until they are marked delivered.

**Tech Stack:** Rust, Diesel Async, Postgres, tokio, testcontainers, autumn-web startup hooks, autumn-harvest idempotent start helper

---

### Task 1: Add Red Tests For Durable Publication

**Files:**
- Modify: `examples/reddit-clone/src/workflows.rs`
- Create or Modify: `examples/reddit-clone/src/outbox.rs`

**Step 1: Write the failing split-store outbox delivery test**

Create a Docker-backed test that:
- creates one Postgres instance with separate logical databases for app data and Harvest data
- applies reddit-clone migrations to the app DB
- applies Harvest migrations to the Harvest DB
- inserts a pending outbox row into the app DB
- runs one drain pass
- asserts the outbox row is marked delivered in app DB
- asserts exactly one Harvest workflow execution exists in the Harvest DB

**Step 2: Run the test to verify it fails**

Run: `cargo test -p reddit-clone durable_outbox_delivery_to_split_harvest_store`

Expected: FAIL because no outbox table or relay exists yet.

**Step 3: Write the failing retry test**

Add a test where the Harvest DB is unavailable during the first drain and verify the outbox row remains pending with an error recorded.

**Step 4: Run the test to verify it fails**

Run: `cargo test -p reddit-clone outbox_retries_failed_delivery`

Expected: FAIL because failures are not yet persisted on outbox rows.

### Task 2: Add App-DB Outbox Schema And Models

**Files:**
- Modify: `examples/reddit-clone/migrations/00000000000000_create_reddit/up.sql`
- Modify: `examples/reddit-clone/src/schema.rs`
- Modify: `examples/reddit-clone/src/models.rs`

**Step 1: Add the outbox table**

Add a `harvest_workflow_outbox` table with:
- `id BIGSERIAL PRIMARY KEY`
- `workflow_name TEXT`
- `workflow_id TEXT`
- `queue_name TEXT`
- `input JSONB`
- `memo JSONB NULL`
- `search_attrs JSONB NULL`
- `delivery_attempts BIGINT DEFAULT 0`
- `last_error TEXT NULL`
- `delivered_execution_id TEXT NULL`
- `delivered_at TIMESTAMP NULL`
- `created_at TIMESTAMP DEFAULT NOW()`

Add an index over pending rows, for example `(delivered_at, id)`.

**Step 2: Add Diesel schema/models**

Add read/write structs so route code and relay code can enqueue and update outbox rows cleanly.

### Task 3: Add Outbox Helpers And Relay

**Files:**
- Create: `examples/reddit-clone/src/outbox.rs`
- Modify: `examples/reddit-clone/src/main.rs`
- Modify: `examples/reddit-clone/src/workflows.rs`

**Step 1: Define a serializable workflow dispatch request**

Represent the exact Harvest start payload needed by onboarding/post-publication.

**Step 2: Add enqueue helpers**

Add helpers that write outbox rows via the app DB.

**Step 3: Add drain helpers**

Drain pending rows by:
- reading a batch from the app DB
- calling Harvest’s `start_or_load_workflow_execution`
- marking successful rows delivered
- incrementing attempts and recording `last_error` on failure

Keep the drain logic safe under duplicate invocation; Harvest idempotency should carry the heavy load.

**Step 4: Add a background relay**

Use `on_startup`/`on_shutdown` in reddit-clone to run a periodic drain loop with a cancellation token.

### Task 4: Route App Writes Through The Outbox

**Files:**
- Modify: `examples/reddit-clone/src/routes/auth.rs`
- Modify: `examples/reddit-clone/src/routes/posts.rs`

**Step 1: Register path**

Wrap the user insert plus onboarding outbox insert in one app-DB transaction.

**Step 2: Post submission path**

Wrap the post insert, author vote insert, and post-publication outbox insert in one app-DB transaction.

**Step 3: Keep latency sane**

After commit, run one best-effort drain pass so normal dev mode still feels immediate.

### Task 5: Verify And Document Remaining Gaps

**Files:**
- Modify: `examples/reddit-clone/src/main.rs` comments if needed
- Modify: `docs/plans/2026-04-09-harvest-topology-roadmap.md` only if wording materially changes

**Step 1: Run verification**

Run:
- `cargo fmt --all`
- `cargo fmt --all` in `autumn-harvest`
- `cargo test -p reddit-clone workflows`
- `cargo test -p reddit-clone --no-run -j 1`

Run Docker-backed coverage:
- `cargo test -p reddit-clone durable_outbox_delivery_to_split_harvest_store`
- `cargo test -p reddit-clone outbox_retries_failed_delivery`

**Step 2: State the remaining caveat honestly**

This slice should make publication durable and retryable, but not instantly synchronous across two stores in every failure mode. Signal dedupe and generalized framework-level outbox support remain future work.
