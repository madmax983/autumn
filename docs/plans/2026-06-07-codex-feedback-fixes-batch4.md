# Codex Feedback Fixes (Batch 4) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Resolve three new Codex PR review feedback items:
1. Guard `set_weights` against removing assigned variants in both memory and postgres stores.
2. Normalize root actuator prefix (handle root `/` or empty as exact-only) before bypassing maintenance mode.
3. Move feature flag checks before the idempotency cache replay in the generated macro.

**Architecture:**
1. In `InMemoryExperimentStore::set_variants` and `PgExperimentStore::set_variants`, add checks verifying that no variants being removed have active assignments. For PostgreSQL, add a concurrency guard (`WHERE NOT EXISTS`) and verify experiment existence if no rows are affected.
2. In `MaintenanceService::gate_request`, perform segment-aware prefix comparison: if the health prefix is `/` or empty, match `/` exactly. Otherwise, match the prefix exactly or as a slash-separated parent path segment.
3. In `autumn-macros/src/feature_flag.rs`, move the `IdempotencyReplayResponse` check and replay code block inside the `flags.enabled` branch of the generated `from_request_parts` implementation.

**Tech Stack:** Rust (2024 edition), Axum, Tower, Diesel, PostgreSQL.

---

### Task 1: Guard variant updates against deleting assigned variants

**Files:**
- Modify: [experiments.rs](file:///c:/Users/markm/autumn/autumn/src/experiments.rs)

**Step 1: Write the failing test**
In `autumn/src/experiments.rs` tests module:
```rust
    #[test]
    fn set_weights_rejects_deleting_assigned_variant() {
        let svc = make_svc();
        running(&svc, "exp");
        let original = svc.assign("exp", "user:1").unwrap();

        let remaining_variant = if original == "control" {
            "treatment"
        } else {
            "control"
        };
        let err = svc
            .set_weights(
                "exp",
                vec![VariantConfig::new(remaining_variant, 100)],
                None,
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot delete variant"),
            "expected active assignment delete guard error, got {err}"
        );
    }
```

**Step 2: Run test to verify it fails**
Run: `cargo test --lib -- experiments::tests::set_weights_rejects_deleting_assigned_variant`
Expected: FAIL (or panic/assertion failure because weights are updated successfully without error).

**Step 3: Write minimal implementation**
1. Modify `InMemoryExperimentStore::set_variants`:
```rust
            let active_variants: std::collections::HashSet<String> = inner
                .assignments
                .values()
                .filter(|a| a.experiment == name)
                .map(|a| a.variant.clone())
                .collect();

            let new_variants: std::collections::HashSet<&str> =
                variants.iter().map(|v| v.name.as_str()).collect();

            for variant in active_variants {
                if !new_variants.contains(variant.as_str()) {
                    return Err(ExperimentStoreError::Backend(format!(
                        "cannot delete variant '{variant}' because it has active assignments"
                    )));
                }
            }
```
2. Modify `PgExperimentStore::set_variants`:
- Load `active_variants` via `diesel::sql_query` and verify them in Rust first:
```rust
            let active_variants = diesel::sql_query(
                "SELECT DISTINCT variant FROM autumn_experiment_assignments WHERE experiment = $1",
            )
            .bind::<diesel::sql_types::Text, _>(name)
            .load::<VariantNameRow>(&mut conn)
            .map_err(|e| ExperimentStoreError::Backend(e.to_string()))?;

            let new_variants: std::collections::HashSet<&str> =
                variants.iter().map(|v| v.name.as_str()).collect();

            for row in active_variants {
                if !new_variants.contains(row.variant.as_str()) {
                    return Err(ExperimentStoreError::Backend(format!(
                        "cannot delete variant '{}' because it has active assignments",
                        row.variant
                    )));
                }
            }
```
- Update `diesel::sql_query` to include a `WHERE NOT EXISTS` subquery check:
```rust
            let rows_affected = diesel::sql_query(
                "WITH updated AS ( \
                     UPDATE autumn_experiments \
                     SET variants = $2::jsonb, updated_at = NOW() \
                     WHERE name = $1 \
                       AND NOT EXISTS ( \
                           SELECT 1 FROM autumn_experiment_assignments a \
                           WHERE a.experiment = name \
                             AND a.variant NOT IN ( \
                                 SELECT x.name FROM jsonb_to_recordset($2::jsonb) AS x(name text) \
                             ) \
                       ) \
                     RETURNING name \
                 ) \
                 INSERT INTO autumn_experiment_changes (experiment, mutation, actor) \
                 SELECT name, 'set_weights', $3 FROM updated",
            )
```
- Check `rows_affected == 0`. If so, run an exists query using `BoolRow` to verify if the experiment exists, and if it does, return `cannot delete variant because it has active assignments`.

**Step 4: Run test to verify it passes**
Run: `cargo test --lib -- experiments::tests::set_weights_rejects_deleting_assigned_variant`
Expected: PASS.

**Step 5: Commit**
`git add autumn/src/experiments.rs`
`git commit -m "fix: guard set_weights against removing assigned variants"`

---

### Task 2: Segment-aware and root-safe maintenance bypass prefix matching

**Files:**
- Modify: [maintenance.rs](file:///c:/Users/markm/autumn/autumn/src/middleware/maintenance.rs)

**Step 1: Write the failing test**
Add two tests in `autumn/src/middleware/maintenance.rs` tests module:
```rust
    #[tokio::test]
    async fn maintenance_on_root_health_prefix_passes_only_root() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());

        let app = Router::new()
            .route("/", get(|| async { "root" }))
            .route("/api/data", get(|| async { "data" }))
            .layer(MaintenanceLayer::new(state).with_health_prefix("/"));

        assert_eq!(response_status(app.clone(), "/").await, StatusCode::OK);
        assert_eq!(response_status(app, "/api/data").await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn maintenance_on_custom_health_prefix_segment_aware() {
        let state = MaintenanceState::new();
        state.enable(MaintenanceConfig::default());

        let app = Router::new()
            .route("/actuator/health", get(|| async { "healthy" }))
            .route("/actuator-dashboard", get(|| async { "dashboard" }))
            .layer(MaintenanceLayer::new(state).with_health_prefix("/actuator"));

        assert_eq!(response_status(app.clone(), "/actuator/health").await, StatusCode::OK);
        assert_eq!(response_status(app, "/actuator-dashboard").await, StatusCode::SERVICE_UNAVAILABLE);
    }
```

**Step 2: Run test to verify it fails**
Run: `cargo test --lib -- middleware::maintenance::tests`
Expected: FAIL (both or one fails, since `/` matches everything and `/actuator` prefix matches `/actuator-dashboard`).

**Step 3: Write minimal implementation**
Modify `MaintenanceService::gate_request`:
```rust
        let path = req.uri().path();
        let health_matched = if self.health_prefix.is_empty() || self.health_prefix == "/" {
            path == "/"
        } else {
            path == self.health_prefix
                || if self.health_prefix.ends_with('/') {
                    path.starts_with(&self.health_prefix)
                } else {
                    let mut prefix_slash = self.health_prefix.clone();
                    prefix_slash.push('/');
                    path.starts_with(&prefix_slash)
                }
        };
        if health_matched {
            return None;
        }
```

**Step 4: Run test to verify it passes**
Run: `cargo test --lib -- middleware::maintenance::tests`
Expected: PASS.

**Step 5: Commit**
`git add autumn/src/middleware/maintenance.rs`
`git commit -m "fix: segment-aware and root-safe maintenance bypass prefix matching"`

---

### Task 3: Move idempotency replay check after the feature flag gate check

**Files:**
- Modify: [feature_flag.rs](file:///c:/Users/markm/autumn/autumn-macros/src/feature_flag.rs)
- Modify: [feature_flags_integration.rs](file:///c:/Users/markm/autumn/autumn/tests/feature_flags_integration.rs)

**Step 1: Write the failing test**
In `autumn/tests/feature_flags_integration.rs`:
- Add handler `macro_gated_idempotent_handler`.
- Add test `feature_flag_checked_before_idempotency_replay`.

```rust
#[post("/macro-gated-idempotent")]
#[feature_flag("replay_flag")]
async fn macro_gated_idempotent_handler() -> &'static str {
    "handler ran"
}

#[tokio::test]
async fn feature_flag_checked_before_idempotency_replay() {
    let store = Arc::new(InMemoryFlagStore::new());
    let shared = SharedStore(store.clone());

    store.enable("replay_flag", None).unwrap();

    let client = TestApp::new()
        .with_flag_store(shared)
        .routes(routes![macro_gated_idempotent_handler])
        .idempotent()
        .build();

    let r1 = client
        .post("/macro-gated-idempotent")
        .header("idempotency-key", "replay-test-key")
        .send()
        .await;
    r1.assert_ok();
    assert_eq!(r1.text(), "handler ran");

    store.disable("replay_flag", None).unwrap();

    let r2 = client
        .post("/macro-gated-idempotent")
        .header("idempotency-key", "replay-test-key")
        .send()
        .await;
    
    assert_eq!(r2.status(), StatusCode::NOT_FOUND.as_u16());
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test feature_flags_integration`
Expected: FAIL (retried request returns 200 "handler ran" via idempotency replay instead of 404).

**Step 3: Write minimal implementation**
Modify the generated `FromRequestParts` implementation in `autumn-macros/src/feature_flag.rs`:
```rust
            async fn from_request_parts(
                parts: &mut ::autumn_web::reexports::http::request::Parts,
                state: &::autumn_web::AppState,
            ) -> ::std::result::Result<Self, Self::Rejection> {
                let flags = <::autumn_web::feature_flags::Flags
                    as ::autumn_web::reexports::axum::extract::FromRequestParts<
                        ::autumn_web::AppState,
                    >>::from_request_parts(parts, state)
                    .await
                    .map_err(|e| {
                        ::autumn_web::reexports::axum::response::IntoResponse::into_response(e)
                    })?;
                if flags.enabled(#flag_key) {
                    if let ::core::option::Option::Some(replay) = parts.extensions.get::<::autumn_web::idempotency::IdempotencyReplayResponse>() {
                        let opt_ext = ::core::option::Option::Some(::autumn_web::reexports::axum::extract::Extension(replay.clone()));
                        if let ::core::option::Option::Some(resp) = ::autumn_web::idempotency::__replay_response(&opt_ext) {
                            return ::std::result::Result::Err(resp);
                        }
                    }
                    ::std::result::Result::Ok(#gate_ident)
                } else {
                    #disabled_rejection
                }
            }
```

**Step 4: Run test to verify it passes**
Run: `cargo test --test feature_flags_integration`
Expected: PASS.

**Step 5: Commit**
`git add autumn-macros/src/feature_flag.rs autumn/tests/feature_flags_integration.rs`
`git commit -m "fix: move idempotency replay check after feature flag gate check"`

---

## Verification Plan

### Automated Tests
- Run `cargo test` to execute all tests in the workspace.
- Run `cargo clippy --all-targets` to verify clean lints.
- Run `cargo fmt` to verify formatting.
