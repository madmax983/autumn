# Reddit Live Feed Observability Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add production-shaped reconnect behavior and observability for the reddit-clone live-feed relay without changing the public live-event delivery model.

**Architecture:** Keep the durable `live_feed_events` table as the source of truth, but give the relay an explicit runtime health object that records wake-source, reconnect, publish, and replay state. The relay should reconnect listeners with bounded backoff after failures instead of silently degrading forever, and that state should be available in-process for tests and operator inspection.

**Tech Stack:** Rust 2024, Tokio, Diesel Async, Redis pub/sub, Postgres `LISTEN/NOTIFY`, Autumn `AppState` typed extensions, serde_json.

---

### Task 1: Define Relay Health Surface

**Files:**
- Modify: `examples/reddit-clone/src/live_events.rs`
- Test: `examples/reddit-clone/src/live_events.rs`

**Step 1: Write the failing test**

Add a test that installs the relay runtime state and asserts the health snapshot reports:
- current listener mode
- wake counts by source
- reconnect attempt count
- last replay cursor / last replayed event timestamp

**Step 2: Run test to verify it fails**

Run: `cargo test -p reddit-clone live_events::tests::relay_health_snapshot_reports_runtime_state -- --exact --nocapture`

Expected: FAIL because no runtime health snapshot exists yet.

**Step 3: Write minimal implementation**

Add a `LiveFeedRelayHealth` extension with atomic counters + lock-protected snapshot fields and helper methods used by the relay and publisher.

**Step 4: Run test to verify it passes**

Run: `cargo test -p reddit-clone live_events::tests::relay_health_snapshot_reports_runtime_state -- --exact --nocapture`

Expected: PASS

### Task 2: Reconnect Dead Listeners

**Files:**
- Modify: `examples/reddit-clone/src/live_events.rs`
- Test: `examples/reddit-clone/src/live_events.rs`

**Step 1: Write the failing test**

Add a test that starts the relay with no initial listener, injects a reconnect hook that succeeds later, and verifies the relay records reconnect attempts then resumes wake-driven rebroadcast without relying on the long poll interval.

**Step 2: Run test to verify it fails**

Run: `cargo test -p reddit-clone live_events::tests::relay_reconnects_after_listener_drop -- --exact --nocapture`

Expected: FAIL because the current relay never heals once the listener is gone.

**Step 3: Write minimal implementation**

Teach the relay loop to:
- track listener health
- attempt reconnects when listener is missing or broken
- use bounded retry interval
- record reconnect success/failure into relay health

**Step 4: Run test to verify it passes**

Run: `cargo test -p reddit-clone live_events::tests::relay_reconnects_after_listener_drop -- --exact --nocapture`

Expected: PASS

### Task 3: Record Publish/Wakeup/Replay Metrics

**Files:**
- Modify: `examples/reddit-clone/src/live_events.rs`
- Test: `examples/reddit-clone/src/live_events.rs`

**Step 1: Write the failing test**

Add focused assertions around:
- Redis publish success/failure counts
- wake counts for Redis / Postgres / poll fallback
- replayed event count and replay lag

**Step 2: Run test to verify it fails**

Run: `cargo test -p reddit-clone live_events::tests::relay_health_tracks_publish_and_wake_sources -- --exact --nocapture`

Expected: FAIL because those counters do not exist yet.

**Step 3: Write minimal implementation**

Update publisher + relay paths to record counts and timestamps in the shared health object without introducing blocking locks across awaits.

**Step 4: Run test to verify it passes**

Run: `cargo test -p reddit-clone live_events::tests::relay_health_tracks_publish_and_wake_sources -- --exact --nocapture`

Expected: PASS

### Task 4: Update Operator Docs

**Files:**
- Modify: `examples/reddit-clone/README.md`

**Step 1: Document the new surface**

Add a short operator section covering:
- what the relay health snapshot measures
- what reconnect behavior looks like
- which wake source is primary vs backup in each topology
- what sustained poll fallback means

**Step 2: Verify docs read cleanly**

Run: `rg -n "live feed relay|poll fallback|Redis|Postgres" examples/reddit-clone/README.md`

Expected: updated wording present and internally consistent.

### Task 5: Final Verification

**Files:**
- Modify: `examples/reddit-clone/src/live_events.rs`
- Modify: `examples/reddit-clone/README.md`

**Step 1: Run focused tests**

Run: `cargo test -p reddit-clone live_events::tests::relay_health_snapshot_reports_runtime_state live_events::tests::relay_reconnects_after_listener_drop live_events::tests::relay_health_tracks_publish_and_wake_sources -- --nocapture`

Expected: PASS

**Step 2: Run broader coverage**

Run: `cargo test -p reddit-clone --lib`

Expected: PASS

**Step 3: Scan for stubs**

Run: `rg -n "TODO|FIXME|Stub:" examples/reddit-clone/src/live_events.rs examples/reddit-clone/README.md`

Expected: no matches
