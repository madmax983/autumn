# Codex Feedback Fixes (Batch 3) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Resolve three new Codex PR review feedback items in the workspace:
1. Preserve resource-level policy denials (`403 Forbidden` / `404 Not Found`) on sunsetted routes guarded by `#[authorize]`.
2. Ensure configured health probes (like `/live`, `/ready`) bypass the maintenance mode `503 Service Unavailable` response.
3. Synchronously read the maintenance flag file on process startup to immediately initialize the maintenance state before serving traffic.

**Architecture:**
1. Rerun authentication checks on sunset routes, but defer the `410 Gone` short-circuit on routes with `#[authorize]` policy checks. Let them run the handler's policy check first, and check for a `SunsetMarker` extension immediately after authorization succeeds.
2. Extend `MaintenanceLayer` and `MaintenanceService` to accept a list of bypass paths, matching them via slash-delimited prefix comparison. Automatically populate this list with the actuator prefix and configured health/probe paths in `router.rs`.
3. Read `tmp/autumn-maintenance.json` synchronously at startup inside `AppBuilder::run` using `MaintenanceState::load_from_file`, populating `MaintenanceState` before building/listening.

**Tech Stack:** Rust (2024 edition), Axum, Tower.

---

### Task 1: Defer sunset 410 short-circuiting on #[authorize] routes to prioritize policy denials

**Files:**
- Modify: [openapi.rs](file:///c:/Users/markm/autumn/autumn/src/openapi.rs)
- Modify: [router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)
- Modify: [lib.rs](file:///c:/Users/markm/autumn/autumn/src/lib.rs)
- Modify: [route.rs](file:///c:/Users/markm/autumn/autumn-macros/src/route.rs)
- Modify: [authorize.rs](file:///c:/Users/markm/autumn/autumn-macros/src/authorize.rs)
- Modify: [api_versioning_integration.rs](file:///c:/Users/markm/autumn/autumn/tests/api_versioning_integration.rs)

**Step 1: Write the failing test**
In `autumn/tests/api_versioning_integration.rs`, add a test validating that when a sunset route has `#[authorize]`, an authenticated user failing authorization receives `403 Forbidden` (or policy result) instead of `410 Gone`, but an authenticated user passing authorization receives `410 Gone`.

```rust
#[autumn_web::prelude::get("/api/v1/sunset-policy-secured/{id}")]
#[autumn_web::prelude::secured]
#[autumn_web::prelude::authorize("show", resource = MockResource)]
async fn sunset_policy_handler(id: Path<i64>) -> &'static str {
    "ok"
}

// Ensure MockResource has a policy that allows only id == 42
struct MockResourcePolicy;
#[async_trait::async_trait]
impl ::autumn_web::authorization::Policy<MockResource> for MockResourcePolicy {
    type Context = ();
    async fn can_show(&self, _ctx: &Self::Context, record: &MockResource) -> bool {
        record.id == 42
    }
}
```

**Step 2: Run test to verify it fails**
Expected: FAIL (unauthorized user gets `410 Gone` instead of `403 Forbidden`).

**Step 3: Write minimal implementation**
1. Add `pub has_policy: bool` to `ApiDoc` in `autumn/src/openapi.rs`.
2. Add `pub has_policy: bool` to `RouteVersionMetadata` in `autumn/src/router.rs`.
3. Re-export `RouteVersionMetadata` and a new public helper `check_sunset` in `autumn/src/lib.rs`'s `__private` module.
4. Define `check_sunset` in `autumn/src/router.rs` that accepts `&AppState` and `&RouteVersionMetadata` and returns `Option<Response>`.
5. Modify `api_versioning_middleware` in `autumn/src/router.rs`:
   - If `is_sunset && !meta.sunset_opt_out`:
     - Rerun authentication checks if `meta.secured`.
     - If `meta.has_policy` is true, call `next.run(request).await` to run the handler.
     - Else, return `check_sunset(&state, &meta)`.
6. Modify `autumn-macros/src/route.rs` to set `has_policy: has_authorize_guard(&input_fn)` in `ApiDoc`.
7. Modify `autumn-macros/src/authorize.rs` to inject `__autumn_route_version: Option<Extension<RouteVersionMetadata>>` extractor parameter, and right after `__check_policy` succeeds, invoke `check_sunset` and return the response if present.

**Step 4: Run test to verify it passes**
Expected: PASS.

**Step 5: Commit**
`git commit -m "fix: preserve policy denials for sunset authorize routes"`

---

### Task 2: Keep configured health probes open in maintenance

**Files:**
- Modify: [maintenance.rs](file:///c:/Users/markm/autumn/autumn/src/middleware/maintenance.rs)
- Modify: [router.rs](file:///c:/Users/markm/autumn/autumn/src/router.rs)
- Modify: [maintenance.rs](file:///c:/Users/markm/autumn/autumn/tests/security/maintenance.rs)

**Step 1: Write the failing test**
In `autumn/tests/security/maintenance.rs`, add a test checking that configured probe paths bypass maintenance mode and return `200 OK` (using `with_bypass_paths`).

**Step 2: Run test to verify it fails**
Expected: FAIL (probes return `503 Service Unavailable`).

**Step 3: Write minimal implementation**
1. Add `bypass_paths: Vec<String>` to `MaintenanceLayer` and `MaintenanceService` in `autumn/src/middleware/maintenance.rs`.
2. Update `gate_request` to check if `path` matches exactly or starts with any of the `bypass_paths` using slash-delimited prefixing.
3. In `autumn/src/router.rs`, populate `bypass_paths` using the actuator prefix and configured health paths from the configuration, passing them to `MaintenanceLayer` via `.with_bypass_paths(...)`.

**Step 4: Run test to verify it passes**
Expected: PASS.

**Step 5: Commit**
`git commit -m "fix: keep configured health probes open in maintenance"`

---

### Task 3: Load maintenance flag synchronously before serving

**Files:**
- Modify: [app.rs](file:///c:/Users/markm/autumn/autumn/src/app.rs)
- Modify: [maintenance.rs](file:///c:/Users/markm/autumn/autumn/tests/security/maintenance.rs)

**Step 1: Write the failing test**
In `autumn/tests/security/maintenance.rs`, add a test case verifying that if the flag file exists with active status, starting a new `MaintenanceState` (or the server) synchronously reads it and starts as active.

**Step 2: Run test to verify it fails**
Expected: FAIL (starts as inactive).

**Step 3: Write minimal implementation**
In `AppBuilder::run` in `autumn/src/app.rs`, synchronously call `MaintenanceState::load_from_file(flag_path)` on startup, and if active, immediately call `maintenance_state.enable(cfg)` before mounting/serving the app.

**Step 4: Run test to verify it passes**
Expected: PASS.

**Step 5: Commit**
`git commit -m "fix: load maintenance flag before serving"`
