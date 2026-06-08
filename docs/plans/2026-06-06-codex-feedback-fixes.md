# Codex Feedback Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Address 4 Codex review feedback items across routing macros, webhook replay keys, panic-safe key release, and CSRF route exemptions in the `madmax983/autumn` workspace.

**Architecture:** Skip primitive wrapping when feature flags are stacked on routes; enforce both delivery ID and signature hash keys for webhook replay protection; implement a `Drop` guard on `webhook_replay_cleanup_middleware` to handle panic unwinding; restrict CSRF exempt path matching to exact or slash-delimited subtrees.

**Tech Stack:** Rust (2024 edition), Axum, Tokio, sha2, hex.

---

### Task 1: Gate primitive wrapper on `!has_feature_flag_attr` in route macro

**Files:**
- Modify: [route.rs](file:///c:/Users/markm/autumn/autumn-macros/src/route.rs#L84-L86)
- Test: [feature_flags_integration.rs](file:///c:/Users/markm/autumn/autumn/tests/feature_flags_integration.rs)

**Step 1: Write the failing test**
In `autumn/tests/feature_flags_integration.rs`, add a stacked handler returning a primitive (e.g. `bool`) and a test validating it compiles and runs correctly:

```rust
#[get("/macro-gated-primitive")]
#[feature_flag("macro_flag")]
async fn macro_gated_primitive_handler() -> bool {
    true
}

#[tokio::test]
async fn feature_flag_macro_primitive_wrapper_stacked() {
    let store = InMemoryFlagStore::new();
    store.enable("macro_flag", None).unwrap();

    let client = TestApp::new()
        .with_flag_store(store)
        .routes(routes![macro_gated_primitive_handler])
        .build();

    let resp = client.get("/macro-gated-primitive").send().await;
    resp.assert_ok();
    assert_eq!(resp.text(), "true");
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test feature_flags_integration`
Expected: Compile failure because the primitive wrapper tries to call `macro_gated_primitive_handler()` without passing the flag gate argument.

**Step 3: Write minimal implementation**
In `autumn-macros/src/route.rs`, update the condition for generating the `primitive_wrapper` to:

```rust
    let primitive_wrapper = if should_stringify_primitive_output(&input_fn.sig.output) && !has_feature_flag_attr {
```

**Step 4: Run test to verify it passes**
Run: `cargo test --test feature_flags_integration`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-macros/src/route.rs autumn/tests/feature_flags_integration.rs
git commit -m "fix: gate primitive route wrappers on !has_feature_flag_attr"
```

---

### Task 2: Bind both delivery ID and signature hash in webhook replay keys

**Files:**
- Modify: [webhook.rs](file:///c:/Users/markm/autumn/autumn/src/webhook.rs)
- Test: [signed_webhooks.rs](file:///c:/Users/markm/autumn/autumn/tests/signed_webhooks.rs)

**Step 1: Write the failing test**
In `autumn/tests/signed_webhooks.rs`, write a test that sends a duplicate delivery ID but with a different body/signature and verifies that it is blocked. Also write a test that sends a modified delivery ID but with the same body/signature (replay) and verifies it is blocked.

```rust
// In autumn/tests/signed_webhooks.rs
// Test 1: duplicate-ID replay (same ID, different/tampered body) is rejected.
// Test 2: modified-ID replay (different ID, same signature/body) is rejected.
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test signed_webhooks`
Expected: FAIL (one or both replay attempts succeed).

**Step 3: Write minimal implementation**
1. Update `WEBHOOK_REPLAY_KEY` task-local type to store `Option<(Arc<dyn WebhookReplayStore>, Vec<String>)>`:
```rust
tokio::task_local! {
    pub static WEBHOOK_REPLAY_KEY: std::sync::Arc<std::sync::Mutex<Option<(std::sync::Arc<dyn WebhookReplayStore>, Vec<String>)>>>;
}
```
2. Modify `verify_request` to generate and check two keys:
   - `{provider}:{name}:id:{delivery_id}`
   - `{provider}:{name}:sig:{sig_hash}`
```rust
        let mut keys_to_check = vec![format!(
            "{}:{}:id:{delivery_id}",
            endpoint.config.provider.as_str(),
            endpoint.config.name
        )];
        if matches!(
            endpoint.config.provider,
            WebhookProvider::Github | WebhookProvider::Generic
        ) {
            let sig_hdr = signature_header(endpoint);
            if let Some(sig_val) = headers.get(sig_hdr).and_then(|v| v.to_str().ok()) {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(sig_val.as_bytes());
                let sig_hash = hex::encode(hasher.finalize());
                keys_to_check.push(format!(
                    "{}:{}:sig:{sig_hash}",
                    endpoint.config.provider.as_str(),
                    endpoint.config.name
                ));
            }
        }
```
Check and insert each key. If any returns `false`, return `WebhookVerifyError::DuplicateDelivery`. Populate `WEBHOOK_REPLAY_KEY` with all inserted keys.
3. Update `webhook_replay_cleanup_middleware` to handle a `Vec<String>` of keys and remove them.

**Step 4: Run test to verify it passes**
Run: `cargo test --test signed_webhooks`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/webhook.rs autumn/tests/signed_webhooks.rs
git commit -m "sec: reject both duplicate-ID and modified-ID replays in webhooks"
```

---

### Task 3: Panic-safe webhook replay key release

**Files:**
- Modify: [webhook.rs](file:///c:/Users/markm/autumn/autumn/src/webhook.rs)
- Test: [signed_webhooks.rs](file:///c:/Users/markm/autumn/autumn/tests/signed_webhooks.rs)

**Step 1: Write the failing test**
In `autumn/tests/signed_webhooks.rs`, write a test where the webhook handler panics. Verify that the replay keys are still released and can be retried.

**Step 2: Run test to verify it fails**
Run: `cargo test --test signed_webhooks`
Expected: FAIL (re-sending the webhook fails with DuplicateDelivery after the panic).

**Step 3: Write minimal implementation**
In `webhook_replay_cleanup_middleware`, implement a custom `ReplayKeyGuard` struct implementing `Drop` that unloads the keys and spawns a background tokio task to remove them if `completed` is false:

```rust
    struct ReplayKeyGuard {
        cell: std::sync::Arc<std::sync::Mutex<Option<(std::sync::Arc<dyn WebhookReplayStore>, Vec<String>)>>>,
        completed: bool,
    }

    impl Drop for ReplayKeyGuard {
        fn drop(&mut self) {
            if !self.completed {
                let to_remove = {
                    let mut guard = self.cell.lock().unwrap();
                    guard.take()
                };
                if let Some((store, keys)) = to_remove {
                    tokio::spawn(async move {
                        for key in keys {
                            tracing::debug!(key = %key, "Releasing webhook replay key due to panic");
                            let _ = store.remove(&key).await;
                        }
                    });
                }
            }
        }
    }
```

**Step 4: Run test to verify it passes**
Run: `cargo test --test signed_webhooks`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/webhook.rs autumn/tests/signed_webhooks.rs
git commit -m "fix: release webhook replay keys on handler panic using Drop guard"
```

---

### Task 4: Restrict CSRF exemptions to exact or slash-delimited subtrees

**Files:**
- Modify: [csrf.rs](file:///c:/Users/markm/autumn/autumn/src/security/csrf.rs)
- Test: [csrf.rs](file:///c:/Users/markm/autumn/autumn/src/security/csrf.rs)

**Step 1: Write the failing test**
Add a test in `csrf.rs` where `/webhooks/stripe-admin` is called, and assert it is NOT exempted when `/webhooks/stripe` is in `exempt_paths`.

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web security::csrf::tests`
Expected: FAIL (the request to `/webhooks/stripe-admin` is exempted and succeeds).

**Step 3: Write minimal implementation**
In `csrf.rs`, inside `CsrfService::call`, change `is_exempt` check to verify that `path == prefix` or `prefix.ends_with('/')` or `path.strip_prefix(prefix)` starts with a slash:

```rust
        let is_exempt = self
            .settings
            .exempt_paths
            .iter()
            .any(|prefix| {
                if path == prefix {
                    true
                } else if let Some(stripped) = path.strip_prefix(prefix) {
                    prefix.ends_with('/') || stripped.starts_with('/')
                } else {
                    false
                }
            });
```

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web security::csrf::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/security/csrf.rs
git commit -m "sec: exempt only exact paths or slash-delimited subtrees from CSRF"
```
