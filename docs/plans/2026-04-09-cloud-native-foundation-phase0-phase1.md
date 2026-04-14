# Cloud-Native Foundation Phase 0 And Phase 1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Deliver the first real cloud-native Autumn milestone by fixing runtime/config drift, adding explicit probe contracts, making OpenTelemetry first-class, externalizing production session state, and shipping container-first scaffolding from the CLI.

**Architecture:** Do not attempt a big-bang rewrite. Phase 0 removes lies already present in the runtime so operators can trust what Autumn says. Phase 1 adds the minimum deployment contract for orchestrated environments: explicit probes, first-class telemetry, pluggable production-safe session backends, and generated container packaging. Keep Autumn monolith-first and local-first; use external state only where distributed correctness actually requires it.

**Tech Stack:** Rust 1.86+, edition 2024, Axum, Tokio, Tower, Diesel + diesel-async, tracing, tracing-subscriber, OpenTelemetry + OTLP, Prometheus-compatible metrics, Redis for the first external session backend, Docker multi-stage builds, testcontainers where integration coverage is required.

**Related ADRs:** `docs/adr/0001-adopt-cloud-native-foundation.md`, `docs/adr/0002-adopt-probe-lifecycle-contracts.md`, `docs/adr/0003-adopt-first-class-opentelemetry.md`, `docs/adr/0004-externalize-distributed-runtime-state.md`

---

## Design Decisions Baked Into This Plan

### DD-1: Fix Drift Before Adding Surface Area

Do not add new cloud-native features on top of misleading runtime behavior. Existing config that is not honored end-to-end must either become real or be removed before new platform claims land.

### DD-2: Cloud-Native Monolith, Not Microservice Theater

The target is a single deployable Autumn service that behaves correctly under orchestration. Nothing in this plan requires service meshes, RPC frameworks, or decomposition into multiple services.

### DD-3: Production-Safe Must Be Explicit

If a default is only safe for development or single-instance use, the docs and runtime should say so plainly. Autumn should stop quietly pretending process-local memory is fine everywhere.

### DD-4: Durable Async Work Is Harvest Territory

Phase 0/1 should improve task and telemetry boundaries, but not turn `#[scheduled]` into a fake workflow engine. Durable distributed job semantics belong to Harvest.

---

## Task 1: Remove Existing Config And Runtime Drift

**Files:**
- Modify: `C:\Users\markm\autumn\autumn\src\config.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\db.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\actuator.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\router.rs`
- Test: `C:\Users\markm\autumn\autumn\tests\config_runtime_drift.rs`

**Step 1: Write the failing tests**

Add targeted tests for:
- `actuator.prefix` changing the mounted route prefix
- `database.connect_timeout_secs` being honored by pool construction, or the field being rejected/deprecated if the underlying pool cannot support it safely
- startup transparency output reflecting the real mounted actuator prefix

**Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test -p autumn-web config_runtime_drift -- --nocapture
```

Expected: FAIL because actuator routes are hard-coded under `/actuator/*` and `connect_timeout_secs` is not used by pool creation.

**Step 3: Write minimal implementation**

- Thread the actuator prefix into router construction instead of hard-coding paths
- Either implement connection-acquisition timeout semantics or remove/deprecate the field with a startup warning plus documentation change
- Keep the compatibility surface explicit, not magical

**Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test -p autumn-web config_runtime_drift -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add autumn/src/config.rs autumn/src/db.rs autumn/src/actuator.rs autumn/src/router.rs autumn/tests/config_runtime_drift.rs
git commit -m "fix: align cloud runtime config with actual behavior"
```

## Task 2: Add Explicit Probe Contracts

**Files:**
- Create: `C:\Users\markm\autumn\autumn\src\probe.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\config.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\state.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\health.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\router.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\app.rs`
- Test: `C:\Users\markm\autumn\autumn\tests\probe_contracts.rs`

**Step 1: Write the failing tests**

Cover:
- `GET /live` returns `200` after bind even when readiness fails
- `GET /ready` returns `503` when DB or required dependencies are not ready
- `GET /startup` returns `503` until startup hooks complete
- readiness flips false before shutdown drain begins serving traffic away
- `/health` remains as a compatibility alias during the transition

**Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test -p autumn-web probe_contracts -- --nocapture
```

Expected: FAIL because only `/health` exists today.

**Step 3: Write minimal implementation**

- Add probe path config with sane defaults
- Add probe state to `AppState`
- Separate liveness, readiness, and startup semantics cleanly
- Preserve `/health` as a compatibility endpoint for `v0.x`

**Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test -p autumn-web probe_contracts -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add autumn/src/probe.rs autumn/src/config.rs autumn/src/state.rs autumn/src/health.rs autumn/src/router.rs autumn/src/app.rs autumn/tests/probe_contracts.rs
git commit -m "feat: add explicit liveness readiness and startup probes"
```

## Task 3: Support Extensible Readiness Checks

**Files:**
- Modify: `C:\Users\markm\autumn\autumn\src\app.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\probe.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\state.rs`
- Test: `C:\Users\markm\autumn\autumn\tests\probe_custom_checks.rs`

**Step 1: Write the failing tests**

Cover:
- app-level custom readiness checks can be registered
- multiple checks aggregate into one readiness response
- failed custom checks surface redacted, operator-usable error messages

**Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test -p autumn-web probe_custom_checks -- --nocapture
```

Expected: FAIL because no readiness-check registration seam exists.

**Step 3: Write minimal implementation**

- Add an app-builder registration API for readiness checks
- Keep liveness non-extensible in Phase 1 unless a concrete use case appears
- Make readiness composition cheap and testable

**Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test -p autumn-web probe_custom_checks -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add autumn/src/app.rs autumn/src/probe.rs autumn/src/state.rs autumn/tests/probe_custom_checks.rs
git commit -m "feat: add extensible readiness checks"
```

## Task 4: Add Telemetry Configuration And Subscriber Wiring

**Files:**
- Modify: `C:\Users\markm\autumn\Cargo.toml`
- Modify: `C:\Users\markm\autumn\autumn\Cargo.toml`
- Modify: `C:\Users\markm\autumn\autumn\src\config.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\logging.rs`
- Create: `C:\Users\markm\autumn\autumn\src\telemetry.rs`
- Test: `C:\Users\markm\autumn\autumn\tests\telemetry_config.rs`

**Step 1: Write the failing tests**

Cover:
- telemetry config loads from TOML and env
- OTLP endpoint plus service metadata produce a telemetry runtime config
- invalid OTLP config falls back to normal logging when strict mode is off

**Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test -p autumn-web telemetry_config -- --nocapture
```

Expected: FAIL because no telemetry config or OTLP wiring exists.

**Step 3: Write minimal implementation**

- Add `[telemetry]` config
- Introduce a telemetry module that can build the tracing subscriber stack
- Keep local pretty/json logging viable when OTLP is disabled or fails to initialize

**Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test -p autumn-web telemetry_config -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add Cargo.toml autumn/Cargo.toml autumn/src/config.rs autumn/src/logging.rs autumn/src/telemetry.rs autumn/tests/telemetry_config.rs
git commit -m "feat: add telemetry configuration and otlp initialization"
```

## Task 5: Propagate Trace Context Across Requests, Tasks, And Harvest Boundaries

**Files:**
- Modify: `C:\Users\markm\autumn\autumn\src\app.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\task.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\db.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\telemetry.rs`
- Modify: `C:\Users\markm\autumn\autumn-harvest\autumn-web-harvest\src\ext.rs`
- Test: `C:\Users\markm\autumn\autumn\tests\telemetry_propagation.rs`
- Test: `C:\Users\markm\autumn\autumn-harvest\autumn-web-harvest\tests\api_scheduler_integration.rs`

**Step 1: Write the failing tests**

Cover:
- inbound `traceparent` is extracted for HTTP requests
- response metadata includes trace correlation information
- scheduled tasks inherit parent trace context when spawned from traced request or startup paths
- Harvest integration keeps trace context when handing work from web layer to durable runtime boundaries

**Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test -p autumn-web telemetry_propagation -- --nocapture
cargo test --manifest-path autumn-harvest/Cargo.toml -p autumn-web-harvest api_scheduler_integration -- --nocapture
```

Expected: FAIL because no OTEL propagation exists.

**Step 3: Write minimal implementation**

- Add HTTP extraction/injection
- Wrap scheduler/task spawns with span propagation
- Add DB spans around pool acquisition and query execution where practical
- Keep telemetry optional behind config and feature gating

**Step 4: Run tests to verify they pass**

Run the same commands as Step 2.

Expected: PASS.

**Step 5: Commit**

```powershell
git add autumn/src/app.rs autumn/src/task.rs autumn/src/db.rs autumn/src/telemetry.rs autumn/tests/telemetry_propagation.rs autumn-harvest/autumn-web-harvest/src/ext.rs autumn-harvest/autumn-web-harvest/tests/api_scheduler_integration.rs
git commit -m "feat: propagate telemetry across runtime boundaries"
```

## Task 6: Add Production Session Backends

**Files:**
- Modify: `C:\Users\markm\autumn\Cargo.toml`
- Modify: `C:\Users\markm\autumn\autumn\Cargo.toml`
- Modify: `C:\Users\markm\autumn\autumn\src\config.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\session.rs`
- Modify: `C:\Users\markm\autumn\autumn\src\router.rs`
- Create: `C:\Users\markm\autumn\autumn\src\session_redis.rs`
- Test: `C:\Users\markm\autumn\autumn\tests\session_backends.rs`

**Step 1: Write the failing tests**

Cover:
- config selects `memory` versus `redis` session backends
- shared session store preserves session continuity across separate app instances
- `prod` profile warns or rejects accidental in-memory sessions unless explicitly acknowledged

**Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test -p autumn-web session_backends -- --nocapture
```

Expected: FAIL because the router always installs `MemoryStore`.

**Step 3: Write minimal implementation**

- Add a session backend selection config
- Keep `MemoryStore` as the local-development default
- Add Redis as the first external backend
- Make production-safety behavior explicit instead of silent

**Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test -p autumn-web session_backends -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add Cargo.toml autumn/Cargo.toml autumn/src/config.rs autumn/src/session.rs autumn/src/session_redis.rs autumn/src/router.rs autumn/tests/session_backends.rs
git commit -m "feat: add pluggable production session backends"
```

## Task 7: Generate Container-First Scaffolding From The CLI

**Files:**
- Modify: `C:\Users\markm\autumn\autumn-cli\src\new.rs`
- Modify: `C:\Users\markm\autumn\autumn-cli\src\templates\Cargo.toml.tmpl`
- Modify: `C:\Users\markm\autumn\autumn-cli\src\templates\autumn.toml.tmpl`
- Create: `C:\Users\markm\autumn\autumn-cli\src\templates\Dockerfile.tmpl`
- Create: `C:\Users\markm\autumn\autumn-cli\src\templates\.dockerignore.tmpl`
- Create: `C:\Users\markm\autumn\autumn-cli\tests\cloud_native_scaffold.rs`

**Step 1: Write the failing tests**

Cover:
- `autumn new` emits a Dockerfile and `.dockerignore`
- generated config includes commented probe and telemetry examples
- generated project can build a production image without editing template internals first

**Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test -p autumn-cli cloud_native_scaffold -- --nocapture
```

Expected: FAIL because the generated app does not include container artifacts.

**Step 3: Write minimal implementation**

- Add multi-stage Docker template
- run as non-root in the final image
- keep generated defaults small and readable
- avoid hard-coding Kubernetes-specific manifests in this phase

**Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test -p autumn-cli cloud_native_scaffold -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add autumn-cli/src/new.rs autumn-cli/src/templates/Cargo.toml.tmpl autumn-cli/src/templates/autumn.toml.tmpl autumn-cli/src/templates/Dockerfile.tmpl autumn-cli/src/templates/.dockerignore.tmpl autumn-cli/tests/cloud_native_scaffold.rs
git commit -m "feat: generate cloud-native container scaffolding"
```

## Task 8: Document Production-Safe Defaults And Deployment Guidance

**Files:**
- Modify: `C:\Users\markm\autumn\README.md`
- Create: `C:\Users\markm\autumn\docs\guide\cloud-native.md`
- Modify: `C:\Users\markm\autumn\docs\guide\getting-started.md`
- Modify: `C:\Users\markm\autumn\docs\guide\tutorial\10-configuration.md`
- Modify: `C:\Users\markm\autumn\examples\bookmarks-distributed\README.md`

**Step 1: Write the failing doc checklist**

Checklist:
- docs distinguish local-safe from production-safe defaults
- docs show probe endpoints and telemetry config
- docs explain when to use `#[scheduled]` versus Harvest
- docs explain generated container assets and migration job expectations

**Step 2: Review docs against the checklist and confirm gaps**

Expected: multiple gaps in current README/tutorial/docs.

**Step 3: Write minimal documentation**

- add a cloud-native guide
- tighten README claims
- add explicit production caveats where defaults are local-only

**Step 4: Re-run the checklist**

Expected: checklist complete with no missing items.

**Step 5: Commit**

```powershell
git add README.md docs/guide/cloud-native.md docs/guide/getting-started.md docs/guide/tutorial/10-configuration.md examples/bookmarks-distributed/README.md
git commit -m "docs: add cloud-native deployment guidance"
```

## Task 9: Final Verification And Release Checklist

**Files:**
- Review only

**Step 1: Run targeted Autumn framework verification**

Run:

```powershell
cargo test -p autumn-web probe_contracts probe_custom_checks telemetry_config telemetry_propagation session_backends config_runtime_drift -- --nocapture
```

Expected: PASS.

**Step 2: Run Autumn CLI verification**

Run:

```powershell
cargo test -p autumn-cli cloud_native_scaffold -- --nocapture
```

Expected: PASS.

**Step 3: Run Harvest integration verification**

Run:

```powershell
cargo test --manifest-path autumn-harvest/Cargo.toml -p autumn-web-harvest api_scheduler_integration -- --nocapture
```

Expected: PASS.

**Step 4: Run formatting and lint gates**

Run:

```powershell
cargo fmt --check
cargo clippy -p autumn-web --all-features -- -D warnings
cargo clippy -p autumn-cli -- -D warnings
```

Expected: PASS.

**Step 5: Scan touched areas for stubs**

Run:

```powershell
rg -n "TODO|FIXME|Stub:" autumn autumn-cli autumn-harvest docs
```

Expected: no newly introduced stubs in touched areas.

---

## Recommended Execution Order

1. Task 1 first. Phase 0 is the truth serum.
2. Tasks 2 and 3 next. Probe semantics define the runtime contract.
3. Tasks 4 and 5 after probe semantics stabilize.
4. Task 6 after telemetry groundwork is in place.
5. Task 7 once runtime config and operational contracts are real.
6. Task 8 after code paths settle.
7. Task 9 last, with fresh verification only.

## Risks To Watch During Execution

- Do not break local `cargo run` ergonomics while adding production features.
- Do not make OTLP mandatory for normal logging.
- Do not silently keep in-memory sessions as the production path.
- Do not let `/health`, `/live`, and `/ready` drift into contradictory semantics.
- Do not smuggle Harvest durability promises into `#[scheduled]`.

Plan complete and saved to `docs/plans/2026-04-09-cloud-native-foundation-phase0-phase1.md`.

Two execution options:

1. Subagent-Driven (this session) - dispatch a fresh worker per task, review between tasks.
2. Parallel Session (separate) - open a dedicated execution session and work the plan checkpoint-by-checkpoint.

Recommended: Subagent-Driven, starting with Task 1 and Task 2, because Phase 0 drift and probe semantics are the foundation for everything else.
