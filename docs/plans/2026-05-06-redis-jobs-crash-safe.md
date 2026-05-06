# Redis Jobs Crash-Safe Runtime Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `#[job]` with `jobs.backend = "redis"` at-least-once and crash-safe for multi-replica workers.

**Architecture:** Replace Redis list pop-and-delete with durable job records keyed by id, a ready list, processing sorted set, delayed retry sorted set, and dead-letter list. Workers atomically claim ready ids, ack only after handler completion, promote due retries, and requeue or dead-letter stale processing records after a visibility timeout.

**Tech Stack:** Rust 2024, Tokio, Redis async connection manager, serde JSON, Lua-backed Redis atomic transitions, TDD red/green/refactor.

---

### Task 1: Red Tests

**Files:**
- Modify: `autumn/src/job.rs`
- Modify: `autumn/src/config.rs`

**Steps:**
1. Add tests for durable Redis job metadata, stale claim recovery, delayed retry promotion, terminal dead-lettering, and visibility timeout config.
2. Run focused tests with `cargo test -p autumn-web job::tests --features redis` and `cargo test -p autumn-web config::tests::env_override_jobs_fields`.
3. Confirm they fail because the current Redis runtime deletes work on claim and has no visibility timeout.

### Task 2: Green Runtime

**Files:**
- Modify: `autumn/src/job.rs`
- Modify: `autumn/src/config.rs`

**Steps:**
1. Add Redis record ids and metadata fields: `attempt`, `claimed_by`, `claimed_at_ms`, and `last_error`.
2. Enqueue by writing the record and ready id atomically.
3. Claim ready ids into `processing` using an atomic Lua transition.
4. Ack successful jobs by removing the processing id and deleting the record.
5. On handler failure, schedule the next attempt in a delayed sorted set with exponential backoff; on exhaustion, dead-letter.
6. Reclaim stale processing ids after `jobs.redis.visibility_timeout_ms`.

### Task 3: Docs and Verification

**Files:**
- Modify: `docs/guide/jobs.md`

**Steps:**
1. Document Redis at-least-once semantics, visibility timeout, retry/dead-letter behavior, and idempotent handler requirement.
2. Run `cargo fmt`.
3. Run focused Redis/config tests, then broader `cargo test -p autumn-web --features redis`.
4. Scan affected files for unfinished-work markers before reporting completion.
