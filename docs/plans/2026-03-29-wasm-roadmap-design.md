# Autumn WASM Roadmap Design

Status: Won't do

This plan is retained as historical context only.

Autumn is returning to a server-first scope: assembled crates, SSR/SSG, htmx, static JS, and raw Axum escape hatches. First-class WASM islands/server-actions were dropped before release because they pulled the framework away from that goal and created maintenance cost without a compelling core use case.

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Define a phased WASM roadmap for Autumn that starts with Rust islands and typed server actions, preserves Autumn's server-first identity, and keeps full client-side app mode as a conditional later extension instead of a premature product bet.

**Architecture:** Autumn remains an SSR/SSG framework built around Axum, Maud, and CLI-managed build steps. Phases 0-2 add a narrow browser lane: CLI-managed WASM asset builds, a small `autumn-wasm` browser runtime, macro-generated island and action metadata, and explicit server-side helpers for rendering mount points and loading browser assets. Shared DTOs compile for both server and browser targets; server-only and browser-only code stay separated by target `cfg` and explicit builder registration.

**Tech Stack:** Rust, Axum, Maud, Serde, `wasm-bindgen`, `web-sys`, `wasm-bindgen-cli-support`, `notify_debouncer_mini`, `trybuild`, `wasm-bindgen-test`

---

## Summary

Autumn should not try to win by becoming "Rust React with a trench coat". The strongest product move is narrower and better:

1. Keep SSR and hybrid rendering as the default.
2. Add first-class Rust WASM islands for interactive fragments.
3. Add typed server actions so browser Rust can call server Rust without handwritten fetch glue.
4. Delay app shell and full client mode until islands and actions prove real demand.

This document intentionally designs phases 0-2 in detail and treats phases 3-5 as future extensions gated by real usage.

## Decision Framing

### Reverse Brainstorming: How We Would Ruin This

- Ship "WASM support" that is just static asset passthrough and call it done.
- Force every user into a client runtime when most pages only need SSR or htmx.
- Make `autumn dev` and `autumn build` slower, flakier, and harder to debug than the current flow.
- Hide too much behavior behind macros and destroy Autumn's current error quality.
- Create two frameworks in one repo: Autumn-the-server-framework and Autumn-the-client-framework.
- Break progressive enhancement and turn normal pages into blank shells when WASM is absent.
- Promise "shared Rust everywhere" while still forcing users into stringly endpoints and handwritten JS glue.

### Anti-Goals

- No mandatory SPA runtime.
- No virtual DOM or framework-owned renderer in phase 1.
- No npm requirement for the default WASM path.
- No generated browser magic that cannot be explained with `cargo expand` and clear docs.
- No breaking change to current non-WASM Autumn apps.

### Go / No-Go Gates

- **Gate A (after phase 1):** islands must be useful on their own. If they are awkward or brittle, stop.
- **Gate B (after phase 2):** typed actions must be clearly better than handwritten fetch. If not, stop.
- **Gate C (before phase 4):** only pursue app shell if real examples show that SSR plus islands is insufficient.

## Product Roadmap

| Phase | Outcome | Why it matters | Advance only if |
|------|---------|----------------|-----------------|
| 0 | WASM build and asset foundations | Makes browser Rust a supported lane instead of repo folklore | Tooling is deterministic and does not rot `autumn dev` |
| 1 | Rust islands on SSR pages | Strongest fit with Autumn's identity and current architecture | Islands are ergonomic and progressive by default |
| 2 | Shared models and typed server actions | Real moat: one Rust app, fewer contract mismatches | The generated client/server contract feels simpler than manual fetch |
| 3 | Client primitives and island composition | Makes multiple islands feel like one coherent page | Runtime stays small and non-dogmatic |
| 4 | App shell and hybrid navigation | Unlocks admin and dashboard style apps | This is meaningfully better than SSR plus islands |
| 5 | Full client-side app mode | Conditional endpoint, not the flagship bet | Autumn can beat bring-your-own frontend on ergonomics |

Phases 3-5 are deliberately out of implementation scope for this document. The work here lays down extension points for them without committing Autumn to a second framework.

## Scope

### In Scope

- Phase 0 build, dev, and scaffold changes
- Phase 1 islands API and runtime registration
- Phase 2 typed server actions and shared DTO patterns
- Public API sketches for generated apps
- Proposed crate and module boundaries inside the Autumn workspace

### Out of Scope

- Full client-side routing
- Offline storage or service worker support
- A custom templating or component DSL for browser rendering
- Replacing htmx as a first-class path

## Design Principles

1. **Server-first forever.** Autumn's center of gravity remains server rendering and build-time generation.
2. **Progressive enhancement first.** Every phase 1 page must render useful HTML before any browser code runs.
3. **Thin wrappers, same as today.** New macros generate metadata and obvious glue, not invisible frameworks.
4. **No duplicate truth.** Shared DTOs and action contracts live in Rust once and compile for both targets.
5. **CLI owns toolchain pain.** If WASM support needs special build steps, `autumn-cli` should manage them.
6. **No dirty working tree artifacts.** Browser build outputs go under `target/` in dev and under `dist/` in static builds.

## Generated App Shape

`autumn new --wasm my-app` should keep the mental model to one package, not a surprise workspace explosion.

```text
my-app/
  Cargo.toml
  autumn.toml
  build.rs
  src/
    main.rs          # server app entrypoint
    lib.rs           # shared DTOs, islands, actions, target-neutral exports
    pages.rs         # Maud pages and layouts
    client.rs        # browser islands and client bootstrap
  static/
    css/
  target/
    autumn/
      wasm/
        dev/
        release/
```

### Cargo Shape

The generated package should include:

- a normal server binary target via `src/main.rs`
- a library target (`rlib` + `cdylib`) for shared code and WASM output
- target-specific dependencies for browser-only crates

This keeps the user in "one app" territory while still allowing target-specific code.

## Workspace Changes Inside Autumn

### New Crate

Add `autumn-wasm` to the workspace.

Responsibilities:

- small browser runtime
- island bootstrap and mount registry
- DOM helpers and tiny signal/state primitives for phase 1
- typed action client for phase 2

Non-responsibilities:

- no virtual DOM
- no compiler-driven template language
- no routing in phases 0-2

### Existing Crates

#### `autumn-web`

Add optional `wasm` feature for server-side integration pieces:

- island metadata and registration types
- action metadata and generated endpoint support
- asset manifest loading and serving helpers
- Maud helpers for island mount points and client asset tags

#### `autumn-macros`

Add:

- `#[island]`
- `islands![]`
- `#[server]`
- `actions![]`

These should mirror the current companion-function pattern used by routes and static routes.

#### `autumn-cli`

Extend:

- `autumn new --wasm`
- `autumn dev` dual watch and rebuild for server + browser outputs
- `autumn build` browser build integration and manifest copy
- `autumn setup` validation for the `wasm32-unknown-unknown` Rust target

## Public API: Phase 1 Islands

Phase 1 should make islands explicit in both server and client code.

### Client Code

```rust
use autumn_web::prelude::*;
use autumn_wasm::prelude::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CounterProps {
    pub initial: i32,
}

#[island]
pub fn counter(mut cx: IslandCx<CounterProps>) {
    let count = cx.signal(cx.props().initial);
    cx.on("click", "[data-inc]", move |_| {
        count.update(|value| *value += 1);
    });
    cx.text("[data-count]", move || count.get().to_string());
}

#[autumn_web::client_main]
pub fn client() {
    autumn_wasm::boot(islands![counter]);
}
```

### Server Code

```rust
use autumn_web::prelude::*;
use crate::client::{counter, CounterProps};

#[get("/")]
async fn index() -> Markup {
    html! {
        head {
            (autumn_web::wasm::assets())
        }
        body {
            h1 { "Counter" }
            (autumn_web::wasm::island(
                counter,
                CounterProps { initial: 3 },
                html! {
                    div class="counter" {
                        button type="button" data-inc { "+" }
                        span data-count { "3" }
                    }
                },
            ))
        }
    }
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index])
        .islands(islands![counter])
        .run()
        .await;
}
```

### Why This Shape

- `assets()` is explicit, matching Autumn's existing "no mystery layout runtime" style.
- `island(...)` wraps fallback HTML plus serialized props and mount metadata.
- `islands![]` mirrors `routes![]`, `tasks![]`, and `static_routes![]`.
- Browser code stays in Rust without forcing a renderer abstraction on the whole page.

## Public API: Phase 2 Typed Server Actions

Phase 2 should eliminate handwritten endpoint glue for common browser-to-server interactions.

### Shared DTOs

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenameTodo {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TodoView {
    pub id: i64,
    pub title: String,
}
```

### Action Definition

```rust
#[server]
pub async fn rename_todo(input: RenameTodo) -> AutumnResult<TodoView> {
    // normal server-side Autumn code
    // db/session/auth access stays on the server implementation only
    todo_service::rename(input).await
}
```

### Server Registration

```rust
#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index])
        .actions(actions![rename_todo])
        .run()
        .await;
}
```

### Client Usage

```rust
let updated = rename_todo(RenameTodo {
    id: 7,
    title: "less cursed title".to_owned(),
}).await?;
```

### Generated Behavior

`#[server]` should expand to:

- the original server implementation on non-WASM targets
- a WASM-target async stub with the same Rust signature
- hidden action metadata describing the generated endpoint
- a POST endpoint under `/_autumn/actions/{name}`

The point is not novelty. The point is that the user writes one Rust function and Autumn generates the boring HTTP glue.

## Macro Design

### `#[island]`

The macro should follow the existing Autumn pattern:

- preserve the user's function body
- emit a hidden metadata companion such as `__autumn_island_meta_counter()`
- emit a hidden client registration companion such as `__autumn_island_mount_counter()`

Proposed metadata type:

```rust
pub struct IslandMeta {
    pub name: &'static str,
    pub mount_id: &'static str,
    pub props_type: &'static str,
}
```

The metadata is intentionally small in phase 1. This is registration, not a reflection system.

### `islands![]`

Collect hidden companions into `Vec<IslandMeta>`, exactly the same pattern as `routes![]` and `static_routes![]`.

### `#[server]`

The macro should generate:

- server implementation under `cfg(not(target_arch = "wasm32"))`
- client stub under `cfg(target_arch = "wasm32")`
- hidden metadata companion for registration

Proposed metadata type:

```rust
pub struct ActionMeta {
    pub name: &'static str,
    pub path: &'static str,
}
```

### `actions![]`

Collect hidden action metadata companions into `Vec<ActionMeta>`.

## Runtime Design

### `AppBuilder`

Add two new builder methods:

```rust
pub fn islands(mut self, islands: Vec<crate::wasm::IslandMeta>) -> Self
pub fn actions(mut self, actions: Vec<crate::wasm::ActionMeta>) -> Self
```

App state should hold readonly registries so the runtime can:

- serve the client manifest and browser assets
- register generated action endpoints
- expose enough metadata for diagnostics and future actuator visibility

### Asset Serving

Phase 0 and 1 asset serving should not write browser outputs into `static/`.

Proposed locations:

- dev: `target/autumn/wasm/dev/`
- release build staging: `target/autumn/wasm/release/`
- static output: copied into `dist/static/autumn/`

At runtime:

- `autumn dev` serves browser assets from `target/autumn/wasm/dev/`
- `autumn build` copies browser assets into `dist/static/autumn/`
- the server helper `autumn_web::wasm::assets()` reads the current manifest and emits the right hashed filenames

### Manifest

Add a JSON manifest separate from static HTML routing:

```json
{
  "entry_js": "/static/autumn/client-abc123.js",
  "entry_wasm": "/static/autumn/client-def456_bg.wasm",
  "islands": {
    "counter": {
      "mount_id": "counter"
    }
  }
}
```

Phase 0 only needs one entry bundle. Code splitting can wait.

## Build and Dev Flow

### `autumn setup`

Add WASM validation mode:

- verify `wasm32-unknown-unknown` target is installed
- print exact remediation if it is missing
- do not require network during `cargo build`

### `autumn dev`

Extend current watch routing in `autumn-cli/src/dev.rs`:

- Rust server changes: rebuild server binary and restart
- WASM/shared/client changes: rebuild WASM bundle and trigger browser reload
- shared DTO changes: rebuild both server and WASM outputs

This must remain incremental. If every save causes both targets to rebuild, the user will start hating Autumn on principle.

### `autumn build`

Extend current static build orchestration:

1. build server binary as today
2. build library for `wasm32-unknown-unknown`
3. post-process with `wasm-bindgen-cli-support`
4. emit hashed JS and WASM assets plus manifest
5. run static route rendering
6. copy browser assets into `dist/static/autumn/`

The browser asset pipeline should be optional. If no islands are registered, `autumn build` skips it.

## Data and Security Boundaries

### Shared Code Rules

Shared DTO modules may contain:

- serde types
- validation logic that compiles for both targets
- pure helper functions

Shared DTO modules must not contain:

- Diesel queries
- session access
- direct DOM code

### Action Security

Generated action client stubs must:

- use same-origin credentials
- send CSRF token automatically when Autumn CSRF is enabled
- preserve normal HTTP status handling instead of inventing a second error protocol

Generated action endpoints must:

- be ordinary Axum routes under Autumn's middleware pipeline
- respect auth/session/csrf/security headers exactly like hand-written routes
- support tracing and request IDs

## Testing Strategy

### Compile-Time

- `trybuild` tests for `#[island]`, `islands![]`, `#[server]`, and `actions![]`
- compile-fail cases for non-serializable props, invalid signatures, and missing async on `#[server]`

### Runtime

- unit tests for manifest loading and helper rendering
- integration tests for action registration and endpoint behavior
- integration tests proving pages still render useful HTML without browser code

### Browser

- `wasm-bindgen-test` for the `autumn-wasm` runtime
- one end-to-end example app proving island hydration and typed action roundtrip

## Proposed File Changes

### Autumn Workspace

- Create: `autumn-wasm/Cargo.toml`
- Create: `autumn-wasm/src/lib.rs`
- Create: `autumn-wasm/src/boot.rs`
- Create: `autumn-wasm/src/island.rs`
- Create: `autumn-wasm/src/action.rs`
- Create: `autumn/src/wasm/mod.rs`
- Create: `autumn/src/wasm/manifest.rs`
- Create: `autumn/src/wasm/types.rs`
- Modify: `Cargo.toml`
- Modify: `autumn/Cargo.toml`
- Modify: `autumn/src/lib.rs`
- Modify: `autumn/src/prelude.rs`
- Modify: `autumn/src/app.rs`
- Modify: `autumn-macros/src/lib.rs`
- Create: `autumn-macros/src/island.rs`
- Create: `autumn-macros/src/islands_macro.rs`
- Create: `autumn-macros/src/server_action.rs`
- Create: `autumn-macros/src/actions_macro.rs`
- Modify: `autumn-cli/src/main.rs`
- Modify: `autumn-cli/src/new.rs`
- Modify: `autumn-cli/src/build.rs`
- Modify: `autumn-cli/src/dev.rs`
- Modify: `autumn-cli/src/setup.rs`
- Create: `autumn-cli/src/templates/client.rs.tmpl`
- Create: `autumn-cli/src/templates/lib.rs.tmpl`
- Modify: `autumn-cli/src/templates/Cargo.toml.tmpl`

### Example App

- Create: `examples/wasm-islands/Cargo.toml`
- Create: `examples/wasm-islands/src/main.rs`
- Create: `examples/wasm-islands/src/lib.rs`
- Create: `examples/wasm-islands/src/client.rs`

## Implementation Tasks

### Task 1: Add phase 0 WASM manifest and registration types

**Files:**
- Create: `autumn/src/wasm/mod.rs`
- Create: `autumn/src/wasm/types.rs`
- Create: `autumn/src/wasm/manifest.rs`
- Modify: `autumn/src/lib.rs`
- Modify: `autumn/src/prelude.rs`

**Steps:**

1. Add `IslandMeta`, `ActionMeta`, and `WasmManifest` types.
2. Add manifest load helpers and Maud rendering helpers for `assets()` and `island(...)`.
3. Add unit tests for manifest roundtrips and generated HTML snippets.
4. Run: `cargo test -p autumn-web wasm`

### Task 2: Extend `AppBuilder` for islands and actions

**Files:**
- Modify: `autumn/src/app.rs`
- Modify: `autumn/src/state.rs`
- Modify: `autumn/tests/app_builder.rs`

**Steps:**

1. Add `islands` and `actions` collections to `AppBuilder`.
2. Store registries in app state.
3. Wire asset serving and generated action routes into router construction.
4. Add integration tests proving registration works and does not affect non-WASM apps.
5. Run: `cargo test -p autumn-web app_builder`

### Task 3: Add `#[island]` and `islands![]`

**Files:**
- Create: `autumn-macros/src/island.rs`
- Create: `autumn-macros/src/islands_macro.rs`
- Modify: `autumn-macros/src/lib.rs`
- Add tests under: `autumn/tests/compile-pass/` and `autumn/tests/compile-fail/`

**Steps:**

1. Implement macro parsing and metadata companion generation.
2. Implement the collection macro using the existing hidden-companion pattern.
3. Add compile-pass tests for valid islands.
4. Add compile-fail tests for invalid signatures and unsupported prop types.
5. Run: `cargo test -p autumn-web compile`

### Task 4: Add the `autumn-wasm` browser runtime

**Files:**
- Create: `autumn-wasm/src/lib.rs`
- Create: `autumn-wasm/src/boot.rs`
- Create: `autumn-wasm/src/island.rs`
- Create: `autumn-wasm/src/prelude.rs`

**Steps:**

1. Add a registry boot function and island mount loop.
2. Add minimal DOM/event helpers and tiny signal support.
3. Add `wasm-bindgen-test` smoke tests.
4. Run: `cargo test -p autumn-wasm`

### Task 5: Extend CLI build and dev flows

**Files:**
- Modify: `autumn-cli/src/build.rs`
- Modify: `autumn-cli/src/dev.rs`
- Modify: `autumn-cli/src/setup.rs`

**Steps:**

1. Add WASM target detection and actionable errors.
2. Add browser bundle build path and manifest emission.
3. Add dual-watch rebuild logic with shared-file fanout.
4. Add CLI tests where practical and at least one e2e smoke path.
5. Run: `cargo test -p autumn-cli`

### Task 6: Scaffold `autumn new --wasm`

**Files:**
- Modify: `autumn-cli/src/main.rs`
- Modify: `autumn-cli/src/new.rs`
- Create: `autumn-cli/src/templates/client.rs.tmpl`
- Create: `autumn-cli/src/templates/lib.rs.tmpl`
- Modify: `autumn-cli/src/templates/Cargo.toml.tmpl`

**Steps:**

1. Add a `--wasm` flag to project scaffolding.
2. Generate `lib.rs`, `client.rs`, and target-specific dependencies.
3. Add a starter island example.
4. Extend `autumn-cli/tests/e2e.rs` to validate the scaffold.
5. Run: `cargo test -p autumn-cli e2e`

### Task 7: Add `#[server]` and `actions![]`

**Files:**
- Create: `autumn-macros/src/server_action.rs`
- Create: `autumn-macros/src/actions_macro.rs`
- Modify: `autumn-macros/src/lib.rs`
- Modify: `autumn/src/wasm/mod.rs`
- Modify: `autumn-wasm/src/action.rs`

**Steps:**

1. Implement the server/client dual expansion.
2. Add generated POST endpoint registration under `/_autumn/actions/*`.
3. Add client stub support with same-origin credentials and CSRF integration.
4. Add compile tests and runtime integration tests.
5. Run: `cargo test -p autumn-web action`

### Task 8: Ship one end-to-end example and docs

**Files:**
- Create: `examples/wasm-islands/*`
- Modify: `README.md`
- Add docs under: `docs/guide/`

**Steps:**

1. Add one example with two islands and one typed action.
2. Document shared DTO rules and progressive enhancement expectations.
3. Add a short "when to use htmx vs islands vs actions" guide.
4. Run: `cargo test --workspace`

## Open Questions

1. Should `#[server]` generate JSON-only endpoints in phase 2, or support alternate codecs later?
2. Do we want the browser runtime to expose tiny signals in phase 1, or keep it event-only until phase 3?
3. Should `autumn_web::wasm::assets()` be explicit forever, or should a later layout helper inject it automatically?
4. Do we want one bundle for all islands initially, or per-page bundle splitting in phase 2?

## Recommendation

Start with tasks 1-6 and ship phases 0-1 as a coherent milestone. Then pause and validate the ergonomics before implementing `#[server]`. If phase 1 already feels heavy, stop there and keep Autumn's WASM story focused on islands plus user-managed fetch.
