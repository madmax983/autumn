# Reddit Live Event Bus Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a real pluggable live-event bus for `reddit-clone` so distributed web/runner processes can use a broker-backed fan-out path while keeping the app database event log as replay truth.

**Architecture:** Keep `live_feed_events` as the durable source of truth. Introduce a bus abstraction with two backends: `postgres_notify` for the happy path and `redis_pubsub` for distributed deployments. Writers persist the event row first, then publish a best-effort bus hint carrying the durable event id; web nodes consume the bus and replay rows from the durable log so missed notifications do not lose data.

**Tech Stack:** Rust 2024, Autumn `AppState` runtime extensions, Diesel async Postgres, Redis async pub/sub, testcontainers (Postgres + Redis), Tokio tasks.

---

### Task 1: Add bus config coverage first

**Files:**
- Modify: `examples/reddit-clone/src/lib.rs`
- Modify: `examples/reddit-clone/autumn-split-web.toml`
- Modify: `examples/reddit-clone/autumn-split-runner.toml`

**Step 1: Write the failing tests**

- Add config tests for:
  - default config uses `postgres_notify`
  - split profiles opt into `redis_pubsub`

**Step 2: Run tests to verify they fail**

Run: `cargo test -p reddit-clone live_feed_bus --lib`
Expected: FAIL because no live-feed bus config type/loader exists yet.

**Step 3: Implement minimal config surface**

- Add a loader for `[distributed.live_feed_bus]`
- Default to `postgres_notify` when the section is absent
- Parse `redis_pubsub` profile config cleanly

**Step 4: Run tests to verify they pass**

Run: `cargo test -p reddit-clone live_feed_bus --lib`

### Task 2: Prove broker-backed relay behavior with a failing integration test

**Files:**
- Modify: `examples/reddit-clone/src/live_events.rs`
- Modify: `examples/reddit-clone/Cargo.toml`
- Modify: `Cargo.toml`

**Step 1: Write the failing test**

- Add a Docker-backed test that:
  - starts Postgres and Redis
  - starts a web relay configured for `redis_pubsub`
  - persists an event from a separate runner state
  - publishes the event id through the bus
  - asserts the web `feed` channel receives the event before a long poll fallback could fire

**Step 2: Run test to verify it fails**

Run: `cargo test -p reddit-clone live_events::tests::redis_bus_rebroadcasts_runner_events_into_web_channels -- --nocapture`
Expected: FAIL because the Redis bus backend does not exist.

**Step 3: Add dependencies**

- Enable the Redis module in `testcontainers-modules`
- Add the `redis` crate with async Tokio features in `examples/reddit-clone/Cargo.toml`

**Step 4: Run test again**

Run the same command and confirm it still fails for the missing implementation, not for missing crates.

### Task 3: Implement the live-event bus runtime

**Files:**
- Create: `examples/reddit-clone/src/live_bus.rs`
- Modify: `examples/reddit-clone/src/lib.rs`
- Modify: `examples/reddit-clone/src/live_events.rs`
- Modify: `examples/reddit-clone/src/main.rs`
- Modify: `examples/reddit-clone/src/bin/harvest-runner.rs`
- Modify: `examples/reddit-clone/src/routes/comments.rs`
- Modify: `examples/reddit-clone/src/workflows.rs`

**Step 1: Add the bus types**

- Implement:
  - `LiveFeedBusKind`
  - `LiveFeedBusConfig`
  - `LiveFeedBusPublisher`
  - `LiveFeedBusListener`
- Support:
  - Postgres `LISTEN/NOTIFY`
  - Redis pub/sub

**Step 2: Install a publisher into runtime state**

- On app startup, load the bus config and install `LiveFeedBusPublisher` into `AppState`
- In the standalone runner, install the same publisher into the detached `AppState` before Harvest starts

**Step 3: Split persistence from publication**

- Change live-event persistence to return the durable event id
- Move bus publication out of the DB transaction
- Keep publication best-effort and log failures instead of aborting the user-facing write

**Step 4: Update the relay**

- Replace the hard-coded Postgres listener with the pluggable bus listener
- Keep durable replay by `event_id > cursor`
- Preserve polling fallback when the bus is unavailable

**Step 5: Run focused tests**

Run:
- `cargo test -p reddit-clone live_events::tests::relay_notify_path_beats_long_poll_interval -- --nocapture`
- `cargo test -p reddit-clone live_events::tests::redis_bus_rebroadcasts_runner_events_into_web_channels -- --nocapture`

### Task 4: Document and verify the escape hatch

**Files:**
- Modify: `examples/reddit-clone/README.md`
- Modify: `examples/reddit-clone/docker-compose.yml`

**Step 1: Update local distributed example docs**

- Add Redis service to `docker-compose.yml`
- Document that:
  - embedded/default mode uses Postgres notify
  - split/distributed mode can switch to Redis pub/sub
  - the app DB event log remains the replay source of truth

**Step 2: Run verification**

Run:
- `cargo fmt --all`
- `cargo test -p reddit-clone --lib`
- `cargo test -p reddit-clone --bin reddit-clone-harvest-runner -- --nocapture`
- `cargo test -p reddit-clone --no-run -j 1`

**Step 3: Review the affected area for half-done seams**

Run: `rg -n "TODO|FIXME|Stub:" examples/reddit-clone/src examples/reddit-clone/README.md`
Expected: no new stubs in the live-event bus path.
