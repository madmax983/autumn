# Codex Feedback Fixes (Batch 2) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Address 3 new Codex review feedback items covering:
1. Fail-closed behavior for invalid maintenance proxy configs.
2. Using public `.value()` accessor for CspNonce in generated passkey pages.
3. Making the experiment variant deletion check atomic with the upsert query in Postgres.

**Architecture:**
1. Extend `MaintenanceLayer` and `MaintenanceService` with `trusted_proxies_configured` so it behaves exactly like the rate limiter when configured proxies are invalid (failing closed).
2. Modify the passkey login/registration handler templates in the CLI auth generator to use `nonce.map(|n| n.value().to_owned())` instead of the private tuple index `n.0.clone()`.
3. Add a `WHERE NOT EXISTS` check using `jsonb_to_recordset` directly inside the `ON CONFLICT DO UPDATE` statement for experiment upserts in `PgExperimentStore`.

**Tech Stack:** Rust (2024 edition), Axum, Diesel, PostgreSQL.

---

### Task 1: Fail closed when maintenance proxy entries are invalid

**Files:**
- Modify: [maintenance.rs](file:///c:/Users/markm/autumn/autumn/src/middleware/maintenance.rs)
- Modify: [router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)
- Modify: [maintenance.rs](file:///c:/Users/markm/autumn/autumn/tests/security/maintenance.rs)

**Step 1: Write the failing test**
In `autumn/tests/security/maintenance.rs`, add a test verifying that when `trusted_proxies` is configured but contains no valid IPs, forwarded headers are not trusted (failing closed):

```rust
#[tokio::test]
async fn maintenance_invalid_proxies_fails_closed() {
    let state = MaintenanceState::new();
    state.enable(MaintenanceConfig {
        message: Some("Maintenance Mode Active".to_string()),
        allow_ips: vec!["192.168.1.10".to_string()],
        ..Default::default()
    });

    let app = Router::new().route("/", get(|| async { "Hello" })).layer(
        MaintenanceLayer::new(state)
            .with_trust_forwarded_headers(true)
            .with_trusted_proxies_configured(true)
            .with_trusted_proxies(vec![]),
    );

    let peer: SocketAddr = "203.0.113.11:4000".parse().unwrap();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Forwarded-For", "192.168.1.10")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(peer));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test maintenance`
Expected: FAIL (returns 200 OK because it falls back to trusting the X-Forwarded-For header from any peer).

**Step 3: Write minimal implementation**
1. In `autumn/src/middleware/maintenance.rs`:
   - Add `trusted_proxies_configured: bool` to `MaintenanceLayer` and `MaintenanceService`.
   - Implement `with_trusted_proxies_configured(mut self, configured: bool) -> Self` builder method.
   - Update `extract_client_ip` signature to receive `configured` and pass it to `proxy::extract_client_ip`.
2. In `autumn/src/router.rs`:
   - Pass `.with_trusted_proxies_configured(!config.security.rate_limit.trusted_proxies.is_empty())` when instantiating `MaintenanceLayer`.

**Step 4: Run test to verify it passes**
Run: `cargo test --test maintenance`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/middleware/maintenance.rs autumn/src/router.rs autumn/tests/security/maintenance.rs
git commit -m "sec: fail closed when maintenance proxy configs are invalid"
```

---

### Task 2: Use public CspNonce accessor in generated passkey pages

**Files:**
- Modify: [auth.rs](file:///c:/Users/markm/autumn/autumn-cli/src/generate/auth.rs)

**Step 1: Write the failing test**
Update the generator integration/unit tests (if any) or inspect the generated route files to verify they do not contain `n.0.clone()`.

**Step 2: Run test to verify it fails**
Not strictly required if verified visually or via compilation of mock auth generators, but we can verify `autumn-cli` compiles correctly.

**Step 3: Write minimal implementation**
In `autumn-cli/src/generate/auth.rs`:
- On line 4244, change `let script_nonce = nonce.map(|n| n.0.clone());` to `let script_nonce = nonce.map(|n| n.value().to_owned());`.
- On line 4416, change `let script_nonce = nonce.map(|n| n.0.clone());` to `let script_nonce = nonce.map(|n| n.value().to_owned());`.

**Step 4: Run test to verify it passes**
Run: `cargo check -p autumn-cli`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn-cli/src/generate/auth.rs
git commit -m "fix: use public value() accessor for CspNonce in generated passkey pages"
```

---

### Task 3: Make variant deletion check atomic with upsert in experiments

**Files:**
- Modify: [experiments.rs](file:///c:/Users/markm/autumn/autumn/src/experiments.rs)

**Step 1: Write the failing test**
In `autumn/src/experiments.rs` tests (or pg integration tests), verify that an upsert fails if we try to delete a variant that has active assignments.
*(We already have `upsert_rejects_deleting_variant_with_active_assignments` in pg integration tests, but we want to make it atomic).*

**Step 2: Run test to verify it fails**
Run: `cargo test --package autumn-web --lib experiments::tests`
Expected: PASS (but vulnerable to race conditions).

**Step 3: Write minimal implementation**
In `autumn/src/experiments.rs`'s `PgExperimentStore::upsert` implementation:
- Update the SQL query inside `diesel::sql_query` to add a `WHERE NOT EXISTS` condition checking active assignments in the `ON CONFLICT (name) DO UPDATE` clause:
```sql
     ON CONFLICT (name) DO UPDATE SET \
         description = EXCLUDED.description, \
         state = EXCLUDED.state, \
         variants = EXCLUDED.variants, \
         winner = EXCLUDED.winner, \
         exclusion_group = EXCLUDED.exclusion_group, \
         updated_at = NOW() \
     WHERE NOT EXISTS ( \
         SELECT 1 FROM autumn_experiment_assignments a \
         WHERE a.experiment = EXCLUDED.name \
           AND a.variant NOT IN ( \
               SELECT x.name FROM jsonb_to_recordset(EXCLUDED.variants) AS x(name text) \
           ) \
     )
```
- Capture the `rows_affected` from `.execute(&mut conn)`.
- If `rows_affected == 0`, return `Err(ExperimentStoreError::Backend("cannot delete variant because it has active assignments".to_owned()))`.

**Step 4: Run test to verify it passes**
Run: `cargo test --package autumn-web --lib experiments::tests`
Expected: PASS

**Step 5: Commit**
```bash
git add autumn/src/experiments.rs
git commit -m "sec: make variant deletion check atomic with upsert in PgExperimentStore"
```
