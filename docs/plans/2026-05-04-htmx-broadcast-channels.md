# HTMX Broadcast Channels Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add htmx-friendly broadcast helpers, a one-line SSE subscription primitive, channel observability, and config-backed local/Redis channel backends for issue #528.

**Architecture:** Keep the existing `Channels` public API, but move its internals behind a backend trait so the local `tokio::broadcast` registry remains the zero-config default and Redis can fan out messages between replicas. Add a `Broadcast` facade on `AppState` for publishing raw text/bytes and Maud fragments wrapped as htmx out-of-band HTML. Use actuator snapshots from the same registry metrics.

**Tech Stack:** Rust 2024, Axum SSE/WebSocket types, Tokio broadcast, optional Redis pub/sub through the existing `redis` feature, Maud for HTML fragments, Autumn config/session patterns.

---

### Task 1: Public Broadcast API and Channel Metrics

**Files:**
- Modify: `autumn/src/channels.rs`
- Modify: `autumn/src/state.rs`
- Modify: `autumn/src/lib.rs`
- Modify: `autumn/src/prelude.rs`

**Step 1: Write failing tests**

Add tests proving:
- `Broadcast::publish_html(topic, markup)` sends an `hx-swap-oob` envelope.
- `Broadcast::publish(topic, bytes)` sends raw text and rejects invalid UTF-8 bytes.
- `Channels::snapshot()` returns per-topic `subscriber_count`, `lifetime_publish_count`, `dropped_count`, and `lagged_count`.
- `AppState::broadcast()` returns a facade wired to the same channels.

**Step 2: Run tests to verify RED**

Run: `cargo test -p autumn-web channels::tests::broadcast_ --features ws`

Expected: compile failures for missing `Broadcast`, metrics fields, and `AppState::broadcast`.

**Step 3: Implement minimal code**

Add `Broadcast`, `BroadcastError`, `ChannelStats`, metrics tracking, and prelude/root re-exports. Preserve existing `Channels::sender`, `subscribe`, `sse_stream`, and `ChannelMessage` behavior for existing users.

**Step 4: Verify GREEN**

Run: `cargo test -p autumn-web channels::tests --features ws`

### Task 2: SSE Subscription Helper

**Files:**
- Modify: `autumn/src/sse.rs`
- Modify: `autumn/src/channels.rs`

**Step 1: Write failing test**

Add a test showing `autumn_web::sse::stream(&state, "topic")` returns an SSE response over the state channel registry.

**Step 2: Run RED**

Run: `cargo test -p autumn-web sse::tests --features ws`

Expected: compile failure for missing `sse::stream`.

**Step 3: Implement helper**

Expose `sse::stream(state, topic)` as a one-line wrapper around `state.channels().sse_stream(topic)`.

**Step 4: Verify GREEN**

Run: `cargo test -p autumn-web sse::tests --features ws`

### Task 3: Config-Backed Channel Backend Selection

**Files:**
- Modify: `autumn/src/config.rs`
- Modify: `autumn/src/app.rs`
- Modify: `autumn/src/channels.rs`
- Modify: `autumn/src/actuator.rs`

**Step 1: Write failing tests**

Add tests proving:
- `channels.backend` defaults to `in_process`.
- `AUTUMN_CHANNELS__BACKEND`, `AUTUMN_CHANNELS__CAPACITY`, `AUTUMN_CHANNELS__REDIS__URL`, and `AUTUMN_CHANNELS__REDIS__KEY_PREFIX` override config.
- `ConfigProperties` tracks `channels.*`.
- `build_state` uses configured channel capacity/backend.
- Redis backend config without URL is rejected only when selected.

**Step 2: Run RED**

Run: `cargo test -p autumn-web config::tests::channels actuator::tests::configprops_tracks_channels --features ws`

Expected: compile failures for missing config.

**Step 3: Implement local/Redis backend shape**

Add `ChannelBackend`, `ChannelRedisConfig`, `ChannelsBackend` trait, local backend implementation, and Redis pub/sub backend behind `redis`. Wire `build_state` to `Channels::from_config`.

**Step 4: Verify GREEN**

Run: `cargo test -p autumn-web config::tests::channels actuator::tests::configprops_tracks_channels channels::tests --features ws,redis`

### Task 4: Actuator Channels Endpoint

**Files:**
- Modify: `autumn/src/actuator.rs`

**Step 1: Write failing test**

Add a test proving `/actuator/channels` returns the expanded metrics object per topic.

**Step 2: Run RED**

Run: `cargo test -p autumn-web actuator::tests::actuator_channels_returns_metrics --features ws`

Expected: assertion failure because only subscriber counts are returned today.

**Step 3: Implement endpoint shape**

Return `{"channels": {"topic": {"subscriber_count": ..., "lifetime_publish_count": ..., "dropped_count": ..., "lagged_count": ...}}}`.

**Step 4: Verify GREEN**

Run: `cargo test -p autumn-web actuator::tests::actuator_channels_returns_metrics --features ws`

### Task 5: Docs and Example

**Files:**
- Modify: `examples/ws-echo/src/main.rs`
- Modify: `examples/ws-echo/README.md`
- Create: `docs/guide/realtime.md`

**Step 1: Update example**

Extend `ws-echo` with a Maud-rendered live feed route using `Broadcast::publish_html` and an SSE subscription route using `sse::stream`.

**Step 2: Add guide**

Document topics, publishing Maud fragments, SSE subscriptions, config switch to Redis, authorization pattern, actuator observability, and a two-replica Redis reproduction script.

**Step 3: Verify**

Run: `cargo test -p autumn-web --doc --features ws,redis` and `cargo check -p ws-echo`.

### Task 6: Final Gates

**Files:**
- All touched files

**Step 1: Scan for stubs**

Run: `rg -n "TODO|FIXME|Stub:" autumn/src docs/guide examples/ws-echo`

**Step 2: Format**

Run: `cargo fmt --all`

**Step 3: Test**

Run focused tests and a broader check:
- `cargo test -p autumn-web channels::tests --features ws,redis`
- `cargo test -p autumn-web actuator::tests::actuator_channels_returns_metrics --features ws`
- `cargo test -p autumn-web config::tests::channels --features ws,redis`
- `cargo test -p autumn-web --doc --features ws,redis`
- `cargo check -p ws-echo`

**Step 4: Review diff**

Run: `git diff --stat` and `git diff --check`.
