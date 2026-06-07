# 0.5.0 Feedback Bugfixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement bugfixes for 27 Codex review feedback comments on the `madmax983/autumn` workspace, ensuring correctness, test coverage, and clean lint check.

**Architecture:** We group the fixes into cohesive tasks (Security & Cryptography, Feature Flags & Experiments, Web Infrastructure, Data & Persistence, CLI & Scaffolding, and Maintenance Mode) and apply them one by one using a Test-Driven Development (TDD) cycle (write failing test -> pass -> verify -> commit).

**Tech Stack:** Rust (2024 edition), Axum, Diesel, Tokio, tempfile, sha2, hex.

---

### Task 1: Webhook Replay Signed Keys (Issue 1)

**Files:**
- Modify: [autumn/src/webhook.rs](file:///c:/Users/markm/autumn/autumn/src/webhook.rs)

**Step 1: Write the failing test**
In `autumn/tests/signed_webhooks.rs`, write a test where we send a webhook, intercept its body and signature, change the unsigned delivery ID header/JSON field, and verify that it is still rejected as a duplicate.

**Step 2: Run test to verify it fails**
Run: `cargo test --test signed_webhooks`
Expected: FAIL (or it gets accepted with the new delivery ID since the replay key changes).

**Step 3: Write minimal implementation**
In `autumn/src/webhook.rs`, inside `verify_request`, if the provider is `Github` or `Generic`, hash the signature header using SHA-256 and append the hex digest to the `replay_key`.

**Step 4: Run test to verify it passes**
Run: `cargo test --test signed_webhooks`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/webhook.rs autumn/tests/signed_webhooks.rs
git commit -m "sec: hash signature header to bind webhook replay key for Github and Generic providers"
```

---

### Task 2: Flags Session Key (Issue 2)

**Files:**
- Modify: [autumn/src/feature_flags.rs](file:///c:/Users/markm/autumn/autumn/src/feature_flags.rs)

**Step 1: Write the failing test**
Update `feature_flags.rs`'s tests to configure a custom `auth_session_key` (not `"user_id"`) and verify that `from_request_parts` extracts it correctly.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web feature_flags::tests`
Expected: FAIL

**Step 3: Write minimal implementation**
In `autumn/src/feature_flags.rs`'s `FromRequestParts` implementation for `Flags`, replace `session.get("user_id")` with `session.get(state.auth_session_key())`.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web feature_flags::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/feature_flags.rs
git commit -m "fix: use state.auth_session_key() in feature flags FromRequestParts extractor"
```

---

### Task 3: PgExperimentStore::upsert conflict logging (Issue 3)

**Files:**
- Modify: [autumn/src/experiments.rs](file:///c:/Users/markm/autumn/autumn/src/experiments.rs)

**Step 1: Write the failing test**
Add a test in `experiments.rs` where we upsert an experiment twice, and verify that the first logs `'created'` and the second logs `'updated'` in the history.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web experiments::tests`
Expected: FAIL (both log `'created'`).

**Step 3: Write minimal implementation**
- In `PgExperimentStore::upsert`, use `ON CONFLICT (name) DO UPDATE SET ... RETURNING name, (xmax = 0) AS is_insert` and log `'created'` or `'updated'` depending on `is_insert`.
- In `InMemoryExperimentStore::upsert`, check `inner.experiments.contains_key(&name)` to determine if it is an update and log `'updated'` or `'created'` accordingly.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web experiments::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/experiments.rs
git commit -m "fix: correctly log created or updated in PgExperimentStore and InMemoryExperimentStore upsert"
```

---

### Task 4: Stacked Feature Flag & Idempotency (Issues 4 & 5)

**Files:**
- Modify: [autumn-macros/src/route.rs](file:///c:/Users/markm/autumn/autumn-macros/src/route.rs)
- Modify: [autumn-macros/src/feature_flag.rs](file:///c:/Users/markm/autumn/autumn-macros/src/feature_flag.rs)

**Step 1: Write the failing test**
Create a test in `autumn/tests/idempotency_middleware.rs` where a primitive-returning handler (e.g. returns `&'static str` or `String`) is annotated with `#[feature_flag]`, and verify that its output is correctly stringified and idempotency replays work early.

**Step 2: Run test to verify it fails**
Run: `cargo test --test idempotency_middleware`
Expected: FAIL / Compile error

**Step 3: Write minimal implementation**
- In `autumn-macros/src/route.rs`, remove `!has_feature_flag_attr` from the `primitive_wrapper` condition.
- Add `has_feature_flag_attr` to the `body_guarded_replay` condition.
- In `autumn-macros/src/feature_flag.rs`, in the generated `from_request_parts` for the flag gate, look up `IdempotencyReplayResponse` in extensions, and call `__replay_response` to return the replay response early if present.

**Step 4: Run test to verify it passes**
Run: `cargo test --test idempotency_middleware`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-macros/src/route.rs autumn-macros/src/feature_flag.rs
git commit -m "fix: preserve primitive wrapper and early replay check on handlers with feature flags"
```

---

### Task 5: Preserve Catch-all Slashes (Issue 6)

**Files:**
- Modify: [autumn/src/paths.rs](file:///c:/Users/markm/autumn/autumn/src/paths.rs)
- Modify: [autumn-macros/src/route.rs](file:///c:/Users/markm/autumn/autumn-macros/src/route.rs)

**Step 1: Write the failing test**
In `paths.rs`'s tests, test that when formatting a path helper containing a catch-all parameter (starts with `*`), forward slashes in the catch-all parameter are preserved while other characters are percent-encoded.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web paths::tests`
Expected: FAIL (slashes percent-encoded as `%2F`).

**Step 3: Write minimal implementation**
- In `autumn/src/paths.rs`, add a public `encode_catch_all_param` helper that splits on `/`, percent-encodes each segment, and joins them back with `/`.
- In `autumn-macros/src/route.rs`'s `emit_path_helper`, check if the parameter starts with `*` and use `encode_catch_all_param` instead of `encode_path_segment`.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web paths::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/paths.rs autumn-macros/src/route.rs
git commit -m "fix: preserve forward slashes in catch-all path params for typed route helpers"
```

---

### Task 6: CLI Temp Files / Secure Credentials File Generation (Issues 7 & 23)

**Files:**
- Modify: [autumn-cli/Cargo.toml](file:///c:/Users/markm/autumn/autumn-cli/Cargo.toml)
- Modify: [autumn-cli/src/credentials.rs](file:///c:/Users/markm/autumn/autumn-cli/src/credentials.rs)
- Modify: [autumn-cli/src/main.rs](file:///c:/Users/markm/autumn/autumn-cli/src/main.rs)

**Step 1: Write the failing test**
Add a test in `autumn-cli/src/credentials.rs`'s tests showing that the temp file is securely generated with random suffix/prefix and zero-wiped on drop.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-cli credentials::tests`
Expected: FAIL

**Step 3: Write minimal implementation**
- Move `tempfile = "3"` from dev-dependencies to dependencies in `autumn-cli/Cargo.toml`.
- In `autumn-cli/src/credentials.rs`, create a `TempFileGuard` wrapper that implements `Drop` to zero-wipe the file and delete it. Use `tempfile::Builder` to create secure randomized temp files.
- Change `run_edit` to return `Result<(), CredentialsError>`. In `main.rs`, catch the error and call `std::process::exit(1)` so destructors are executed on failure paths.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-cli credentials::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/Cargo.toml autumn-cli/src/credentials.rs autumn-cli/src/main.rs
git commit -m "fix: secure credentials file editing using NamedTempFile and zero-wiping destructor"
```

---

### Task 7: Scaffold Attachment URL-Encoding (Issue 8)

**Files:**
- Modify: [autumn-cli/src/generate/scaffold.rs](file:///c:/Users/markm/autumn/autumn-cli/src/generate/scaffold.rs)

**Step 1: Write the failing test**
Write a test in `autumn-cli` scaffolding generators that asserts form fields representing attachments generate hidden inputs for updates.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-cli generate::scaffold::tests`
Expected: FAIL

**Step 3: Write minimal implementation**
In `scaffold.rs`, render hidden inputs for attachment fields during update operations to keep their values, and support URL-encoding.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-cli generate::scaffold::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/src/generate/scaffold.rs
git commit -m "fix: generate hidden inputs for scaffolded attachments to preserve values on updates"
```

---

### Task 8: Private Key Permissions (Issue 9)

**Files:**
- Modify: [autumn-cli/src/new.rs](file:///c:/Users/markm/autumn/autumn-cli/src/new.rs)
- Modify: [autumn-cli/src/credentials.rs](file:///c:/Users/markm/autumn/autumn-cli/src/credentials.rs)

**Step 1: Write the failing test**
Add a test that creates `master.key` and asserts it is created with `0o600` permissions on Unix.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-cli new::tests`
Expected: FAIL (permissions not set immediately).

**Step 3: Write minimal implementation**
In `new.rs` and `credentials.rs`, use `OpenOptionsExt::mode(0o600)` to create `config/master.key` with `0o600` permissions immediately on Unix.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-cli new::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/src/new.rs autumn-cli/src/credentials.rs
git commit -m "sec: create master.key with 0o600 permissions immediately on Unix"
```

---

### Task 9: Passkey Script / CSP & CSRF (Issues 10 & 25)

**Files:**
- Modify: [autumn-cli/src/generate/auth.rs](file:///c:/Users/markm/autumn/autumn-cli/src/generate/auth.rs)
- Modify: [autumn/src/prelude.rs](file:///c:/Users/markm/autumn/autumn/src/prelude.rs)

**Step 1: Write the failing test**
Update tests for Auth templates to ensure they include nonce in `<script>` tags and map CSRF headers dynamically.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-cli generate::auth::tests`
Expected: FAIL

**Step 3: Write minimal implementation**
- Export `CsrfTokenHeader` in `prelude.rs`.
- In `auth.rs`, update templates for passkeys to accept `Option<CspNonce>` and `CsrfTokenHeader`, and output `<script nonce=[script_nonce]>` and header mapping dynamically.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-cli generate::auth::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/src/generate/auth.rs autumn/src/prelude.rs
git commit -m "fix: add CspNonce and dynamic CSRF header support to passkey templates"
```

---

### Task 10: Exclude master.key from Docker (Issue 12)

**Files:**
- Modify: [autumn-cli/src/templates/.dockerignore.tmpl](file:///c:/Users/markm/autumn/autumn-cli/src/templates/.dockerignore.tmpl)
- Modify: [autumn-cli/src/templates/release/.dockerignore.tmpl](file:///c:/Users/markm/autumn/autumn-cli/src/templates/release/.dockerignore.tmpl)
- Modify: [autumn-cli/src/release.rs](file:///c:/Users/markm/autumn/autumn-cli/src/release.rs)

**Step 1: Write the failing test**
In `autumn-cli/src/release.rs`, add a test `dockerignore_excludes_master_key` to assert `/config/master.key` is in `.dockerignore`.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-cli release::tests::dockerignore_excludes_master_key`
Expected: FAIL

**Step 3: Write minimal implementation**
- Add `/config/master.key` to both `.dockerignore.tmpl` files.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-cli release::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/src/templates/.dockerignore.tmpl autumn-cli/src/templates/release/.dockerignore.tmpl autumn-cli/src/release.rs
git commit -m "sec: ignore config/master.key in Docker builds"
```

---

### Task 11: Rate Limiter Principal Strategy (Issue 13)

**Files:**
- Modify: [autumn/src/router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)

**Step 1: Write the failing test**
Write a test in `autumn/tests/rate_limit_principal.rs` that checks that requests with a session carrying the configured `auth_session_key` are rate-limited based on that user ID principal.

**Step 2: Run test to verify it fails**
Run: `cargo test --test rate_limit_principal`
Expected: FAIL (falls back to IP instead of user ID).

**Step 3: Write minimal implementation**
In `router.rs`, define `populate_rate_limit_principal` middleware that extracts session, looks up the configured `auth_session_key`, inserts `RateLimitPrincipal` into request extensions, and register it inside `apply_rate_limit_middleware` before `RateLimitLayer`.

**Step 4: Run test to verify it passes**
Run: `cargo test --test rate_limit_principal`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/router.rs
git commit -m "fix: insert RateLimitPrincipal from session user ID before rate limiter runs"
```

---

### Task 12: Signed Webhooks CSRF Exemption (Issue 14)

**Files:**
- Modify: [autumn/src/security/csrf.rs](file:///c:/Users/markm/autumn/autumn/src/security/csrf.rs)
- Modify: [autumn/src/router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)

**Step 1: Write the failing test**
Write a test in `autumn/tests/signed_webhooks.rs` that sends a POST request to a webhook path without a CSRF token and asserts that it skips CSRF protection.

**Step 2: Run test to verify it fails**
Run: `cargo test --test signed_webhooks`
Expected: FAIL (returns 403 Forbidden due to missing CSRF token).

**Step 3: Write minimal implementation**
- In `csrf.rs`, implement `with_exempt_path(mut self, path: impl Into<String>) -> Self`.
- In `router.rs`'s `apply_csrf_middleware`, automatically register all configured webhook paths as CSRF exempt.

**Step 4: Run test to verify it passes**
Run: `cargo test --test signed_webhooks`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/security/csrf.rs autumn/src/router.rs
git commit -m "fix: automatically exempt configured webhook endpoints from CSRF verification"
```

---

### Task 13: Release Webhook Replay ID on Failure (Issue 15)

**Files:**
- Modify: [autumn/src/webhook.rs](file:///c:/Users/markm/autumn/autumn/src/webhook.rs)
- Modify: [autumn/src/router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)
- Modify: [autumn/tests/signed_webhooks.rs](file:///c:/Users/markm/autumn/autumn/tests/signed_webhooks.rs)

**Step 1: Write the failing test**
In `tests/signed_webhooks.rs`, write a test where a webhook handler returns a 500 error, and assert that we can resend the same webhook (same delivery ID) again successfully (because the claimed replay key was released).

**Step 2: Run test to verify it fails**
Run: `cargo test --test signed_webhooks`
Expected: FAIL (returns DuplicateDelivery on the second request).

**Step 3: Write minimal implementation**
- In `WebhookReplayStore` trait, add `fn remove<'a>(&'a self, key: &'a str) -> WebhookReplayFuture<'a>;`.
- Implement `remove` for `Arc<T>`, `InMemoryWebhookReplayStore`, `RedisWebhookReplayStore`, and `UnavailableReplayStore` in `tests/signed_webhooks.rs`.
- In `webhook.rs`, declare `tokio::task_local! { pub static WEBHOOK_REPLAY_KEY: std::sync::Arc<std::sync::Mutex<Option<String>>>; }`. Write the claimed key to it on success in `verify_request`.
- In `router.rs`, implement `webhook_replay_cleanup_middleware` task-local wrapper that intercepts 5xx responses and deletes the key from the store. Register it in `apply_csrf_middleware` or similar.

**Step 4: Run test to verify it passes**
Run: `cargo test --test signed_webhooks`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/webhook.rs autumn/src/router.rs autumn/tests/signed_webhooks.rs
git commit -m "fix: release webhook replay key from store if handler returns 5xx server error"
```

---

### Task 14: Install the Maintenance Poller & Trust Forwarded IP Headers (Issues 16 & 17)

**Files:**
- Modify: [autumn/src/security/proxy.rs](file:///c:/Users/markm/autumn/autumn/src/security/proxy.rs) [NEW]
- Modify: [autumn/src/security/mod.rs](file:///c:/Users/markm/autumn/autumn/src/security/mod.rs)
- Modify: [autumn/src/security/rate_limit.rs](file:///c:/Users/markm/autumn/autumn/src/security/rate_limit.rs)
- Modify: [autumn/src/middleware/maintenance.rs](file:///c:/Users/markm/autumn/autumn/src/middleware/maintenance.rs)
- Modify: [autumn/src/router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)
- Modify: [autumn/src/app.rs](file:///c:/Users/markm/autumn/autumn/src/app.rs)

**Step 1: Write the failing test**
In `autumn/tests/maintenance.rs` (or similar), write a test where maintenance is turned on via file flag, and requests are blocked with 503 unless they match a trusted proxy forwarded IP in the allow list.

**Step 2: Run test to verify it fails**
Run: `cargo test --test maintenance`
Expected: FAIL

**Step 3: Write minimal implementation**
- Create `autumn/src/security/proxy.rs` and move `TrustedProxy` struct definition and implementation there. Export it in `autumn/src/security/mod.rs`.
- Import and use `TrustedProxy` in `rate_limit.rs`.
- In `middleware/maintenance.rs`, update `MaintenanceLayer` and `MaintenanceService` to store/pass `trust_forwarded_headers` and `trusted_proxies`. Rewrite `extract_client_ip` using `TrustedProxy`.
- Register `MaintenanceLayer` automatically in `router.rs` using `config.security.rate_limit` proxy settings.
- In `app.rs`'s `AppBuilder::run`, instantiate `MaintenanceState::new()`, insert it as app extension, and start the tokio background poller task looking at `tmp/autumn-maintenance.json`.

**Step 4: Run test to verify it passes**
Run: `cargo test --test maintenance`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/security/proxy.rs autumn/src/security/mod.rs autumn/src/security/rate_limit.rs autumn/src/middleware/maintenance.rs autumn/src/router.rs autumn/src/app.rs
git commit -m "feat: install maintenance mode poller and support trusted proxy forwarded IP headers"
```

---

### Task 15: Keep Experiment Variants Compatible (Issue 18)

**Files:**
- Modify: [autumn/src/experiments.rs](file:///c:/Users/markm/autumn/autumn/src/experiments.rs)

**Step 1: Write the failing test**
Write a test in `experiments.rs` where we assign an actor to a variant in `autumn_experiment_assignments`, then attempt to upsert/update the experiment config without that variant name. Verify it returns an error.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web experiments::tests`
Expected: FAIL (variant is deleted and update succeeds).

**Step 3: Write minimal implementation**
In both `PgExperimentStore::upsert` and `InMemoryExperimentStore::upsert`, query active variant assignments for the experiment. If any active variant name is missing from the new config's variants, reject the update with a `Backend` error.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web experiments::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/experiments.rs
git commit -m "fix: reject updates that remove or rename experiment variants with active assignments"
```

---

### Task 16: Generate CspNonce for Custom CSPs (Issue 19)

**Files:**
- Modify: [autumn/src/security/headers.rs](file:///c:/Users/markm/autumn/autumn/src/security/headers.rs)

**Step 1: Write the failing test**
Add a test in `headers.rs` where we configure a custom `content_security_policy` and enable `csp_nonce`, and verify that `CspNonce` is still generated in request extensions while the custom CSP header is sent unmodified.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web security::headers::tests`
Expected: FAIL

**Step 3: Write minimal implementation**
In `headers.rs`, update `ComputedHeaders` to keep `csp_nonce_enabled` as a boolean. In `SecurityHeadersService::call`, always generate and insert `CspNonce` into request extensions if `csp_nonce_enabled` is true, while only performing placeholder substitution in `poll` when the default template is used.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web security::headers::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/security/headers.rs
git commit -m "fix: always generate and insert CspNonce if enabled even with custom CSP templates"
```

---

### Task 17: Reuse Existing Master Key (Issue 20)

**Files:**
- Modify: [autumn-cli/src/credentials.rs](file:///c:/Users/markm/autumn/autumn-cli/src/credentials.rs)

**Step 1: Write the failing test**
Write a test in `credentials.rs` where we have an existing `master.key` file, create credentials for a new environment, and assert that the existing `master.key` is not replaced or truncated.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-cli credentials::tests`
Expected: FAIL

**Step 3: Write minimal implementation**
In `credentials.rs`, in `edit_credentials`, check `resolve_master_key` first before generating a new key to ensure any existing key is reused.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-cli credentials::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/src/credentials.rs
git commit -m "fix: reuse existing master.key on new credentials environment generation to prevent keys truncation"
```

---

### Task 18: Preserve Authorization Checks on Sunsetted Routes (Issue 21)

**Files:**
- Modify: [autumn/src/router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)

**Step 1: Write the failing test**
Write a test where a sunsetted route marked with `#[secured]` or `#[authorize]` is called, and assert that it returns `401 Unauthorized` / `403 Forbidden` if auth is absent/invalid instead of returning `410 Gone`.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web router::tests`
Expected: FAIL (returns 410 Gone immediately without checking auth).

**Step 3: Write minimal implementation**
In `router.rs`'s `api_versioning_middleware`, if the route is sunsetted but requires authorization/secured, perform the auth checks before returning `410 Gone`.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web router::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/router.rs
git commit -m "fix: enforce authorization checks on sunsetted routes before short-circuiting with 410 Gone"
```

---

### Task 19: Length-Delimit Upload Signature Fields (Issue 22)

**Files:**
- Modify: [autumn/src/storage/local.rs](file:///c:/Users/markm/autumn/autumn/src/storage/local.rs)

**Step 1: Write the failing test**
Write a test in `local.rs` demonstrating that signature collision is impossible between different blob keys and content types (e.g. key `a:b` + type `c` vs key `a` + type `b:c`).

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web storage::local::tests`
Expected: FAIL (same signature produced for both).

**Step 3: Write minimal implementation**
In `local.rs`'s `sign_upload`, prefix `blob_key` and `content_type` with their length as big-endian `u64` values and use `expires_at.to_be_bytes()` for Hmac updates.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web storage::local::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/storage/local.rs
git commit -m "sec: length-delimit signature fields in sign_upload to prevent signature collision bypasses"
```

---

### Task 20: Record History for Hooked Bulk Deletes (Issue 24)

**Files:**
- Modify: [autumn-macros/src/repository.rs](file:///c:/Users/markm/autumn/autumn-macros/src/repository.rs)

**Step 1: Write the failing test**
In `autumn/tests/repository_bulk_operations.rs` (or similar), write a test calling `delete_many` on a hooked, versioned repository and assert that entries are successfully written to `_autumn_version_history`.

**Step 2: Run test to verify it fails**
Run: `cargo test --test repository_bulk_operations`
Expected: FAIL (no history records written).

**Step 3: Write minimal implementation**
In `autumn-macros/src/repository.rs`'s hooked `delete_many_body`, construct `vh_delete_write` and insert it into the transaction block to write history records to the `_autumn_version_history` table.

**Step 4: Run test to verify it passes**
Run: `cargo test --test repository_bulk_operations`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-macros/src/repository.rs
git commit -m "fix: write version history entries for hooked delete_many repository calls"
```

---

### Task 21: Generate a Runnable Mailer Smoke Test (Issue 26)

**Files:**
- Modify: [autumn-cli/src/generate/mailer.rs](file:///c:/Users/markm/autumn/autumn-cli/src/generate/mailer.rs)

**Step 1: Write the failing test**
Write a generator test asserting that the generated mailer smoke test doesn't contain a `#[cfg(feature = "mail")]` gate so it runs immediately.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-cli generate::mailer::tests`
Expected: FAIL

**Step 3: Write minimal implementation**
In `autumn-cli/src/generate/mailer.rs`'s `render_smoke_test`, remove the `#[cfg(feature = "mail")]` attribute gate.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-cli generate::mailer::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/src/generate/mailer.rs
git commit -m "fix: remove cfg(feature = \"mail\") gate from scaffolded mailer smoke tests"
```

---

### Task 22: Reject Partial Tenant Upserts (Issue 27)

**Files:**
- Modify: [autumn-macros/src/repository.rs](file:///c:/Users/markm/autumn/autumn-macros/src/repository.rs)

**Step 1: Write the failing test**
Write a test in `autumn/tests/repository_bulk_operations.rs` calling `upsert_many` with a mixture of records belonging to the current tenant and another tenant, and assert that the operation returns a `Tenant conflict` error instead of silently omitting the other tenant's records.

**Step 2: Run test to verify it fails**
Run: `cargo test --test repository_bulk_operations`
Expected: FAIL (succeeds silently, only upserting current tenant's records).

**Step 3: Write minimal implementation**
In `autumn-macros/src/repository.rs`'s `upsert_many_body`, in the `size_check` block, change `else if !has_lock && upserted.is_empty() && !records.is_empty()` to `else if !has_lock && upserted.len() != records.len()`.

**Step 4: Run test to verify it passes**
Run: `cargo test --test repository_bulk_operations`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-macros/src/repository.rs
git commit -m "fix: reject partial upserts in tenant-scoped upsert_many to avoid silent omissions"
```

---

## Verification Plan

We will verify every task during execution using its corresponding unit/integration test, and run the full workspace test suite and lints at the end.

### Automated Tests
- Run full test suite: `cargo test --workspace`
- Run clippy check: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
