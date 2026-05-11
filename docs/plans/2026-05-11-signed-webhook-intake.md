# Signed Webhook Intake Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add first-class signed webhook intake for Stripe, GitHub, Slack, and generic HMAC-SHA256 callbacks.

**Architecture:** Add an additive `autumn_web::webhook` module with provider presets, raw-body verification, replay protection, and an Axum extractor. Configure endpoints under `security.webhooks.endpoints`, install a `WebhookRegistry` into `AppState`, and let handlers receive a `SignedWebhook` only after verification succeeds.

**Tech Stack:** Rust 2024, Axum extractors, `hmac`/`sha2`, `subtle`, `serde`, Autumn Problem Details responses, in-memory replay store for the built-in default.

---

### Task 1: RED - Contract Tests

**Files:**
- Create: `autumn/tests/signed_webhooks.rs`
- Modify later: `autumn/src/webhook.rs`, `autumn/src/lib.rs`, `autumn/src/prelude.rs`, `autumn/src/security/config.rs`, `autumn/src/config.rs`, `autumn/src/app.rs`, `autumn/src/test.rs`

**Steps:**
1. Add tests for valid Stripe/GitHub/Slack/generic HMAC requests.
2. Add tests proving raw byte changes fail even when JSON is semantically equivalent.
3. Add tests for missing, malformed, stale, bad-signature, duplicate-delivery, and previous-secret rotation cases.
4. Add config tests for production rejecting missing/weak webhook secrets.
5. Run `cargo test -p autumn-web --test signed_webhooks` and confirm RED from missing API.

### Task 2: GREEN - Core Intake API

**Files:**
- Create: `autumn/src/webhook.rs`
- Modify: `autumn/src/lib.rs`, `autumn/src/prelude.rs`

**Steps:**
1. Implement provider enum, endpoint config builders, runtime registry, in-memory replay store, verification error type, and `SignedWebhook`.
2. Implement exact raw-body HMAC bases:
   - Stripe: `timestamp.raw_body`, `Stripe-Signature: t=...,v1=...`
   - GitHub: raw body, `X-Hub-Signature-256: sha256=...`
   - Slack: `v0:timestamp:raw_body`, `X-Slack-Signature: v0=...`
   - Generic: raw body, configurable header/prefix defaults
3. Implement `FromRequest<AppState>` so handler logic runs only after verification.
4. Return `AutumnError` values that render as Problem Details.

### Task 3: GREEN - Config and Startup Wiring

**Files:**
- Modify: `autumn/src/security/config.rs`, `autumn/src/config.rs`, `autumn/src/app.rs`, `autumn/src/test.rs`

**Steps:**
1. Add `security.webhooks.endpoints` config and secret-env resolution.
2. Validate configured endpoint secrets, including previous secrets during production rotation.
3. Install `WebhookRegistry` into `AppState` in normal app startup and `TestApp`.
4. Re-run focused tests and fix compile/test failures.

### Task 4: REFACTOR - Docs and Example

**Files:**
- Create: `docs/guide/signed-webhooks.md`
- Modify: `docs/guide/tutorial/12-whats-next.md`, `docs/guide/tutorial/index.md` if needed
- Create: `examples/signed-webhooks/Cargo.toml`, `examples/signed-webhooks/src/main.rs`, `examples/signed-webhooks/tests/fixtures.rs`
- Modify: root `Cargo.toml`

**Steps:**
1. Document raw-body preservation, provider headers, timestamp tolerance, replay protection, secret sourcing, rotation, error responses, and logging posture.
2. Add a runnable example with fixture tests for valid, tampered-body, stale-timestamp, bad-signature, and duplicate-delivery cases.
3. Run `cargo test -p signed-webhooks-example` plus focused Autumn tests.

### Task 5: Final Verification

**Steps:**
1. Run `cargo fmt`.
2. Run focused tests: `cargo test -p autumn-web --test signed_webhooks`.
3. Run example tests: `cargo test -p signed-webhooks-example`.
4. Scan affected areas for stubs: `rg "TODO|FIXME|Stub:" autumn/src/webhook.rs autumn/tests/signed_webhooks.rs docs/guide/signed-webhooks.md examples/signed-webhooks`.
5. Review `git diff --check` and `git diff --stat`.
