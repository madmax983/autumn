# Sprint Plan: autumn

**Date:** 2026-03-20
**Scrum Master:** markm
**Project Level:** 4
**Total Stories:** 45
**Total Points:** 155
**Planned Sprints:** 12 (2-week sprints)
**Target Completion:** 2026-09-11 (v0.1 on crates.io)

---

## Executive Summary

This plan breaks Autumn's 11 epics and 48 functional requirements into 45 implementable stories across 12 two-week sprints. The plan follows the critical path from the product brief: foundation (proc macros) → database → rendering → production defaults → ship. Stories are ordered by dependency chain, not priority alone — the route system must work before anything else.

**Key Metrics:**

| Metric | Value |
|--------|-------|
| Total Stories | 45 |
| Total Points | 155 |
| Sprints | 12 |
| Capacity/Sprint | 12 pts (conservative), 15 pts (stretch) |
| Total Capacity | 144-180 pts |
| Buffer | ~15% (if velocity averages 13 pts) |
| 3-Month Gut Check | End of Sprint 6 (2026-06-12) |

---

## Team Capacity

| Parameter | Value |
|-----------|-------|
| Developer | 1 (Mark, senior) |
| Hours/week | ~15 (side project cadence) |
| Sprint length | 2 weeks (10 workdays) |
| Productive hours/sprint | ~30 |
| Story point rate | 1 pt ≈ 2.5 hours |
| Capacity/sprint | 12 pts (conservative) |
| ADHD factor | Parallel workstreams available when blocked (config, static assets, CLI scaffolding) |

---

## Story Inventory

### EPIC-002: Route System (28 pts, 7 stories) — HIGHEST RISK

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-001 | Workspace skeleton & crate structure | 2 | FR-004 | None | 1 |
| S-002 | Basic #[get] route macro (no extractors, &str return) | 8 | FR-005 | S-001 | 1-2 |
| S-003 | #[post], #[put], #[delete] macros | 3 | FR-005 | S-002 | 2 |
| S-004 | Auto-apply debug_handler in debug builds | 2 | FR-006 | S-002 | 2 |
| S-005 | routes![] collection macro | 5 | FR-007 | S-002 | 2 |
| S-006 | Path parameter extraction in route macros | 3 | FR-012 | S-002 | 3 |
| S-007 | Proc macro compile_error! diagnostics | 5 | FR-005, NFR-006 | S-002 | 3 |

### EPIC-003: Application Bootstrap (13 pts, 4 stories)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-008 | #[autumn::main] macro (tokio::main wrapper) | 3 | FR-008 | S-001 | 3 |
| S-009 | App builder: routes collection → Axum router | 5 | FR-008 | S-005 | 3 |
| S-010 | Startup route logging + empty-routes panic | 2 | FR-008 | S-009 | 3 |
| S-011 | Request ID middleware | 3 | FR-030 | S-009 | 4 |

### EPIC-005: Error Handling (13 pts, 4 stories) — HIGH RISK

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-012 | AutumnError type + blanket From<E: Error> | 5 | FR-016, FR-017 | S-001 | 4 |
| S-013 | Status code refinement methods (not_found, bad_request, etc.) | 3 | FR-018 | S-012 | 4 |
| S-014 | IntoResponse impl for AutumnError (JSON error body) | 3 | FR-016 | S-012 | 4 |
| S-015 | AutumnResult<T> type alias + handler return type contract | 2 | FR-019 | S-012 | 4 |

### EPIC-004: Database Layer (18 pts, 4 stories) — MEDIUM RISK

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-016 | diesel-async connection pool (deadpool) | 5 | FR-009 | S-009 | 5 |
| S-017 | Db extractor (FromRequestParts impl) | 5 | FR-010 | S-012, S-016 | 5 |
| S-018 | #[derive(Model)] macro | 5 | FR-011 | S-001 | 5 |
| S-019 | Database URL from config integration | 3 | FR-009 | S-016, S-025 | 6 |

### EPIC-006: Rendering Stack (14 pts, 5 stories)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-020 | Maud re-export + Markup as handler return type | 3 | FR-020 | S-012 | 6 |
| S-021 | Tailwind build.rs template (CLI detection + CSS output) | 5 | FR-021 | None | 6 |
| S-022 | htmx embedding (include_bytes!) + serving route | 3 | FR-022 | S-009 | 7 |
| S-023 | Form<T> extractor re-export with Autumn error handling | 2 | FR-013 | S-012 | 7 |
| S-024 | Static input.css with Tailwind directives | 1 | FR-021 | S-021 | 7 |

### EPIC-007: Configuration & Defaults (18 pts, 6 stories)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-025 | AutumnConfig struct with serde defaults | 3 | FR-024, FR-026 | None | 7 |
| S-026 | TOML config file loading (autumn.toml) | 3 | FR-024 | S-025 | 7 |
| S-027 | Environment variable overrides (AUTUMN_* prefix) | 3 | FR-025 | S-025 | 8 |
| S-028 | Structured logging (tracing-subscriber setup) | 3 | FR-027 | S-025 | 8 |
| S-029 | Health check endpoint (/health with pool status) | 3 | FR-028 | S-016 | 8 |
| S-030 | Graceful shutdown (SIGTERM + Ctrl+C) | 3 | FR-029 | S-009 | 8 |

### EPIC-008: JSON & Static Assets (4 pts, 2 stories)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-031 | Json<T> re-export + response type integration | 2 | FR-014, FR-015 | S-012 | 7 |
| S-032 | Static directory serving (tower-http ServeDir) | 2 | FR-023 | S-009 | 7 |

### Prelude & Integration (2 pts, 1 story)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-033 | autumn::prelude module with all re-exports | 2 | FR-004 | S-020, S-031 | 8 |

### EPIC-001: Project Scaffolding & CLI (14 pts, 4 stories)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-034 | autumn-cli crate with clap (--help, --version) | 3 | FR-001 | None | 9 |
| S-035 | autumn new project generation from templates | 5 | FR-002 | S-034 | 9 |
| S-036 | autumn setup: Tailwind CLI download with checksums | 3 | FR-003 | S-034 | 10 |
| S-037 | E2E: generated project compiles + runs + serves HTML | 3 | FR-002 | S-035 | 10 |

### EPIC-009: Documentation & Examples (24 pts, 5 stories)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-038 | README with quickstart + maturity warning | 3 | FR-031 | All Must Have | 10 |
| S-039 | Getting started guide (zero to running app) | 5 | FR-032 | S-038 | 11 |
| S-040 | Tutorial: build a todo app with Autumn | 8 | FR-033 | S-039 | 11 |
| S-041 | Example todo application (in-repo) | 5 | FR-034 | All Must Have | 10 |
| S-042 | API docs: cargo doc with examples on all public types | 5 | FR-035 | All Must Have | 11 |

### EPIC-010: CI & Distribution (7 pts, 3 stories)

| ID | Title | Pts | FRs | Dependencies | Sprint |
|----|-------|-----|-----|--------------|--------|
| S-043 | GitHub Actions CI (3 OS, fmt, clippy, test) | 3 | FR-036 | S-001 | 9 |
| S-044 | Cargo feature flags (tailwind, htmx, maud) | 2 | FR-038 | S-020, S-022 | 9 |
| S-045 | crates.io publication (metadata, license, publish) | 2 | FR-037 | All | 12 |

---

## Sprint Allocation

### Sprint 1 (Mar 23 – Apr 4) — 12/12 pts

**Goal:** Get the first annotated route handler compiling and responding to HTTP requests.

| Story | Title | Pts |
|-------|-------|-----|
| S-001 | Workspace skeleton & crate structure | 2 |
| S-002 | Basic #[get] route macro (no extractors, &str return) | 8 |
| S-025 | AutumnConfig struct with serde defaults | 2* |

**Notes:** S-002 is the hardest single story in the project. If it takes longer than expected, S-025 is the "productive procrastination" task — pure data structures, no macro dependency. Config is on the critical path but is independent work.

*S-025 is 3pts but only the struct definition is needed here (2pts partial); loading comes in S-026.

**Risk:** HIGH — proc macro development is unpredictable. This sprint is deliberately light (12pts) to absorb overrun.

**Done when:** `#[get("/hello")] async fn hello() -> &'static str { "hello" }` compiles and the generated code is correct (even if not yet runnable as a server).

---

### Sprint 2 (Apr 7 – Apr 18) — 13/12 pts

**Goal:** Complete the route macro system: all HTTP methods, debug_handler, and routes![] collection.

| Story | Title | Pts |
|-------|-------|-----|
| S-003 | #[post], #[put], #[delete] macros | 3 |
| S-004 | Auto-apply debug_handler in debug builds | 2 |
| S-005 | routes![] collection macro | 5 |
| S-025 | AutumnConfig struct (remaining 1pt if not done) | 1* |
| S-026 | TOML config file loading | 2* |

**Notes:** Once #[get] works (S-002), the other methods are straightforward copies with different method names. routes![] is the second critical macro. Config loading (S-026) is parallelizable work if macros are going well.

**Done when:** `routes![hello, goodbye]` expands to a `Vec<Route>` and compiles.

---

### Sprint 3 (Apr 21 – May 2) — 13/12 pts

**Goal:** First running server — routes are discovered, mounted, and respond to HTTP requests.

| Story | Title | Pts |
|-------|-------|-----|
| S-006 | Path parameter extraction in route macros | 3 |
| S-008 | #[autumn::main] macro | 3 |
| S-009 | App builder: routes → Axum router → server | 5 |
| S-010 | Startup route logging + empty-routes panic | 2 |

**Notes:** This is the "it's alive" sprint. S-009 is where Autumn becomes a framework — the builder that wires routes into a running Axum server.

**Done when:** Write `#[autumn::main]`, call `autumn::app().routes(my_routes).run().await`, visit `http://localhost:3000/hello/world`, see a response. First running Autumn application.

**🎯 MILESTONE: Autumn serves HTTP responses.**

---

### Sprint 4 (May 5 – May 16) — 13/12 pts

**Goal:** Error handling and request infrastructure — `?` works in handlers, every request gets an ID.

| Story | Title | Pts |
|-------|-------|-----|
| S-007 | Proc macro compile_error! diagnostics | 5 |
| S-012 | AutumnError + blanket From<E: Error> | 5 |
| S-011 | Request ID middleware | 3 |

**Notes:** S-007 is polish on the macro system — making errors human-readable. S-012 is the second riskiest design piece (error handling). Can work on them in parallel (different crates).

**Done when:** A handler with `async fn get() -> AutumnResult<&'static str>` can use `?` on any Error type. Missing `async` on a handler produces "handlers must be async functions" not a wall of trait bounds.

---

### Sprint 5 (May 19 – May 30) — 15/12 pts (stretch)

**Goal:** Database layer — handlers can query Postgres via `db: Db`.

| Story | Title | Pts |
|-------|-------|-----|
| S-013 | Status code refinement methods | 3 |
| S-014 | IntoResponse for AutumnError (JSON) | 3 |
| S-015 | AutumnResult<T> type alias | 2 |
| S-016 | diesel-async connection pool | 5 |
| S-018 | #[derive(Model)] macro | 2* |

**Notes:** Error handling polish (S-013, S-014, S-015) wraps up the error system. S-016 starts the database layer. S-018 can start in parallel (it's a proc macro, independent crate). Stretch sprint — 15pts — because error handling polish stories are well-defined.

*S-018 started, may carry into Sprint 6.

**Done when:** Errors return JSON with correct status codes. Database pool creates and connects.

---

### Sprint 6 (Jun 2 – Jun 13) — 14/12 pts (stretch)

**Goal:** Full database integration + Maud rendering. Handlers query DB and return HTML.

| Story | Title | Pts |
|-------|-------|-----|
| S-017 | Db extractor (FromRequestParts) | 5 |
| S-018 | #[derive(Model)] macro (remaining) | 3* |
| S-019 | Database URL from config | 3 |
| S-020 | Maud re-export + Markup return type | 3 |

**Notes:** This is the sprint where Autumn becomes a real full-stack framework. A handler can take `db: Db`, query Diesel, and return Maud HTML.

**Done when:** `#[get("/users")] async fn list(db: Db) -> AutumnResult<Markup> { ... }` queries the database and returns styled HTML.

**🎯 MILESTONE: 3-MONTH GUT CHECK**
- ✅ Do the proc macros work?
- ✅ Does the Db extractor work?
- ❓ Would you use this yourself? (honest assessment)

If any answer is NO: scope cuts happen immediately (see product brief).

---

### Sprint 7 (Jun 16 – Jun 27) — 13/12 pts

**Goal:** Complete rendering stack — Tailwind CSS, htmx, static assets, JSON responses.

| Story | Title | Pts |
|-------|-------|-----|
| S-021 | Tailwind build.rs template | 5 |
| S-022 | htmx embedding + serving route | 3 |
| S-023 | Form<T> extractor with Autumn errors | 2 |
| S-031 | Json<T> response type integration | 2 |
| S-024 | Static input.css with Tailwind directives | 1 |

**Notes:** After this sprint, the full rendering stack works: Maud templates with Tailwind classes produce styled HTML, htmx is served automatically, forms work, JSON endpoints work.

**Done when:** A page rendered with `html!{ div class="bg-blue-500 p-4" { "styled" } }` actually has blue background. htmx `hx-post` submits work. `Json<Vec<User>>` returns JSON.

---

### Sprint 8 (Jun 30 – Jul 11) — 14/12 pts (stretch)

**Goal:** Production defaults — logging, health check, graceful shutdown. Framework "feels complete."

| Story | Title | Pts |
|-------|-------|-----|
| S-027 | Environment variable overrides | 3 |
| S-028 | Structured logging setup | 3 |
| S-029 | Health check endpoint | 3 |
| S-030 | Graceful shutdown | 3 |
| S-032 | Static directory serving | 2 |
| S-033 | autumn::prelude module | 2* |

**Notes:** All straightforward integration work. No proc macros, no hard design problems. Each story is independent. Good sprint for steady velocity after the hard middle.

*S-033 may need to wait until all re-exports are finalized.

**Done when:** `GET /health` returns pool status. SIGTERM drains connections. Every request gets a trace span with a request ID. Static files serve from `/static/`.

**🎯 MILESTONE: Feature-complete framework (minus CLI and docs).**

---

### Sprint 9 (Jul 14 – Jul 25) — 11/12 pts

**Goal:** CLI scaffolding + CI pipeline. The "developer experience" sprint.

| Story | Title | Pts |
|-------|-------|-----|
| S-034 | autumn-cli crate with clap | 3 |
| S-035 | autumn new project generation | 5 |
| S-043 | GitHub Actions CI (3 OS) | 3 |

**Notes:** CLI is cuttable (fallback: cargo-generate template). CI should be set up early-ish to catch cross-platform issues. S-044 (feature flags) moved here if time.

**Done when:** `autumn new my-app` generates a project. CI runs on all 3 platforms.

---

### Sprint 10 (Jul 28 – Aug 8) — 14/12 pts (stretch)

**Goal:** CLI completion + example app + README. The "it's real" sprint.

| Story | Title | Pts |
|-------|-------|-----|
| S-036 | autumn setup: Tailwind CLI download | 3 |
| S-037 | E2E: generated project compiles + runs | 3 |
| S-041 | Example todo application | 5 |
| S-038 | README with quickstart + maturity warning | 3 |

**Notes:** S-037 is the critical integration test: `autumn new test-app && cd test-app && cargo run` must work. The example app (S-041) is both documentation and integration test.

**Done when:** A stranger can read the README, run the quickstart, and have a working app. The example todo app demonstrates all core features.

**🎯 MILESTONE: Autumn is demoable to the outside world.**

---

### Sprint 11 (Aug 11 – Aug 22) — 13/12 pts

**Goal:** Documentation — getting started guide, tutorial, API docs.

| Story | Title | Pts |
|-------|-------|-----|
| S-039 | Getting started guide | 5 |
| S-042 | API docs (cargo doc) on all public types | 5 |
| S-044 | Cargo feature flags | 2 |
| S-040 | Tutorial: build a todo app (start) | 1* |

**Notes:** The tutorial (S-040) is the largest documentation story at 8pts. It starts here but carries into Sprint 12.

**Done when:** A developer can follow the getting started guide end-to-end. `cargo doc --open` shows documented API with examples.

---

### Sprint 12 (Aug 25 – Sep 5) — 9/12 pts

**Goal:** Ship v0.1 to crates.io.

| Story | Title | Pts |
|-------|-------|-----|
| S-040 | Tutorial: build a todo app (remaining ~7pts) | 7 |
| S-045 | crates.io publication | 2 |

**Notes:** Light sprint on purpose. The tutorial carries over from Sprint 11. Publication is the final step. Buffer exists for any fixes discovered during documentation writing.

**Done when:** `cargo add autumn` works. The tutorial is complete. v0.1 is on crates.io.

**🎯 MILESTONE: v0.1 SHIPPED. 🚀**

---

## Sprint Velocity Plan

| Sprint | Dates | Committed | Theme | Key Milestone |
|--------|-------|-----------|-------|---------------|
| 1 | Mar 23 – Apr 4 | 12 | Foundation: workspace + first macro | |
| 2 | Apr 7 – Apr 18 | 13 | Route system complete | |
| 3 | Apr 21 – May 2 | 13 | First running server | ✨ HTTP responses |
| 4 | May 5 – May 16 | 13 | Error handling + diagnostics | |
| 5 | May 19 – May 30 | 15 | Database pool + error polish | |
| 6 | Jun 2 – Jun 13 | 14 | DB extractor + Maud | 🔍 3-Month Gut Check |
| 7 | Jun 16 – Jun 27 | 13 | Tailwind + htmx + JSON | |
| 8 | Jun 30 – Jul 11 | 14 | Production defaults | ✅ Feature-complete |
| 9 | Jul 14 – Jul 25 | 11 | CLI + CI | |
| 10 | Jul 28 – Aug 8 | 14 | Example app + README | 🌍 Demoable |
| 11 | Aug 11 – Aug 22 | 13 | Documentation | |
| 12 | Aug 25 – Sep 5 | 9 | Tutorial + publish | 🚀 v0.1 shipped |
| | | **154** | | |

**Budget: 154 committed / 144 conservative capacity / 180 stretch capacity**

The plan slightly exceeds conservative capacity (154 vs 144). This is intentional — later sprints (9-12) contain work that's less risky and faster to execute (docs, CI, publication). The hard work (proc macros, error handling, database) is front-loaded where velocity uncertainty is highest, with lower sprint commitments.

---

## Parallel Workstreams (ADHD-Friendly Context Switching)

When blocked on the hard stuff (proc macros, error handling), these stories can be worked on independently:

| When blocked on... | Switch to... | Story |
|---------------------|-------------|-------|
| Route macros (S-002) | Config struct | S-025 |
| Route macros (S-002) | Config loading | S-026 |
| Error handling (S-012) | Tailwind build.rs | S-021 |
| Database pool (S-016) | Static serving | S-032 |
| Any proc macro | CLI scaffolding | S-034 |
| Any runtime code | CI pipeline | S-043 |
| Everything | Health check | S-029 |

---

## Epic Traceability

| Epic ID | Epic Name | Stories | Total Points | Sprints |
|---------|-----------|---------|--------------|---------|
| EPIC-002 | Route System | S-001 to S-007 | 28 | 1-3 |
| EPIC-003 | Application Bootstrap | S-008 to S-011 | 13 | 3-4 |
| EPIC-005 | Error Handling | S-012 to S-015 | 13 | 4-5 |
| EPIC-004 | Database Layer | S-016 to S-019 | 18 | 5-6 |
| EPIC-006 | Rendering Stack | S-020 to S-024 | 14 | 6-7 |
| EPIC-007 | Config & Defaults | S-025 to S-030 | 18 | 1,2,7-8 |
| EPIC-008 | JSON & Static Assets | S-031 to S-032 | 4 | 7-8 |
| — | Prelude & Integration | S-033 | 2 | 8 |
| EPIC-001 | Project Scaffolding | S-034 to S-037 | 14 | 9-10 |
| EPIC-009 | Documentation | S-038 to S-042 | 26 | 10-12 |
| EPIC-010 | CI & Distribution | S-043 to S-045 | 7 | 9,11,12 |
| | **Total** | **45** | **155** | |

---

## Requirements Coverage

All 38 Must Have FRs are covered:

| FR | Name | Story |
|----|------|-------|
| FR-001 | CLI Installation | S-034 |
| FR-002 | Project Scaffolding | S-035, S-037 |
| FR-003 | Tool Management | S-036 |
| FR-004 | Crate Structure | S-001 |
| FR-005 | Route Macros | S-002, S-003, S-007 |
| FR-006 | Debug Handler | S-004 |
| FR-007 | routes![] Macro | S-005 |
| FR-008 | Entry Point | S-008, S-009, S-010 |
| FR-009 | DB Pool | S-016, S-019 |
| FR-010 | Db Extractor | S-017 |
| FR-011 | #[derive(Model)] | S-018 |
| FR-012 | Path Extractor | S-006 |
| FR-013 | Form Extractor | S-023 |
| FR-014 | JSON Extractor | S-031 |
| FR-015 | JSON Response | S-031 |
| FR-016 | AutumnError | S-012, S-014 |
| FR-017 | Blanket From | S-012 |
| FR-018 | Custom Status | S-013 |
| FR-019 | Return Type Contract | S-015 |
| FR-020 | Maud Integration | S-020 |
| FR-021 | Tailwind Pipeline | S-021, S-024 |
| FR-022 | htmx Integration | S-022 |
| FR-023 | Static Assets | S-032 |
| FR-024 | Config File | S-025, S-026 |
| FR-025 | Env Overrides | S-027 |
| FR-026 | Defaults | S-025 |
| FR-027 | Logging | S-028 |
| FR-028 | Health Check | S-029 |
| FR-029 | Graceful Shutdown | S-030 |
| FR-030 | Request ID | S-011 |
| FR-031 | README | S-038 |
| FR-032 | Getting Started | S-039 |
| FR-033 | Tutorial | S-040 |
| FR-034 | Example App | S-041 |
| FR-035 | API Docs | S-042 |
| FR-036 | CI | S-043 |
| FR-037 | crates.io | S-045 |
| FR-038 | Feature Flags | S-044 |

**Should Have FRs (v1.0):** FR-039 through FR-048 are not allocated. They are post-v0.1 work.

---

## Risks and Mitigation

**High:**
- **Proc macro development takes longer than estimated** (S-002: 8pts) — Mitigation: Sprint 1 is deliberately light. Parallel workstreams available. If proc macros aren't working by Sprint 4, simplify to explicit handler registration.
- **Error handling specialization workaround fails** (S-012) — Mitigation: The blanket `From` + explicit `.map_err()` approach is designed to work on stable Rust. Fallback: require users to define error enums (more boilerplate, still functional).
- **Creator loses focus** (product brief risk #1) — Mitigation: Each sprint ends with a visible reward. Parallel workstreams for context switching. 3-month gut check forces honest assessment.

**Medium:**
- **diesel-async integration issues** (S-016, S-017) — Mitigation: Fallback to `spawn_blocking` with sync Diesel. Test against real Postgres in CI from Sprint 5.
- **Tailwind CLI doesn't scan Maud templates correctly** (S-021) — Mitigation: May need custom `--content` glob patterns or a preprocessing step. Spike during Sprint 7.
- **Tutorial takes longer than 8pts** (S-040) — Mitigation: Tutorial spans Sprints 11-12 with buffer. Can ship v0.1 with README + getting started guide only, add tutorial as a fast-follow.

**Low:**
- **Cross-platform CI failures** (S-043) — Mitigation: Windows is the most likely issue. Test early (Sprint 9) to catch platform-specific bugs.
- **crates.io publication blocked** — Mitigation: Pre-register crate names early. Verify metadata requirements before Sprint 12.

---

## Definition of Done

For a story to be considered complete:
- [ ] Code implemented and compiling on stable Rust
- [ ] `cargo fmt` produces no changes
- [ ] `cargo clippy` (pedantic) produces no warnings
- [ ] Unit tests written and passing
- [ ] Integration tests where applicable
- [ ] No `unwrap()` in library code
- [ ] Doc comments on all new public items
- [ ] No regressions in existing tests

---

## Key Decision Points

| Date | Sprint | Decision |
|------|--------|----------|
| Apr 18 | End of Sprint 2 | Do route macros and routes![] work? If not: simplify or extend timeline. |
| Jun 13 | End of Sprint 6 | **3-MONTH GUT CHECK.** Macros? Db? Would you use it? If no: scope cuts. |
| Jul 11 | End of Sprint 8 | Feature-complete? If not: cut CLI (use cargo-generate), cut tutorial, ship with README only. |
| Sep 5 | End of Sprint 12 | Ship or don't. If not ready: push to Sprint 13-14, max 1 month delay. |

---

## Next Steps

**Immediate:** Begin Sprint 1.

Start with **S-001 (Workspace skeleton)** — this is the foundation everything else builds on. Once the workspace compiles, move to **S-002 (Basic #[get] macro)** — the hardest and most important story in the project.

**Options:**
1. `/bmad:create-story S-001` — Create detailed story document for workspace setup
2. `/bmad:dev-story S-001` — Start implementing immediately
3. `/bmad:workflow-status` — Check current status

**Recommended:** Start with `/bmad:dev-story S-001` — the workspace skeleton is well-defined enough to implement directly.

---

**This plan was created using BMAD Method v6 - Phase 4 (Implementation Planning)**

*To continue: Run `/bmad:workflow-status` to see your progress and next recommended workflow.*
