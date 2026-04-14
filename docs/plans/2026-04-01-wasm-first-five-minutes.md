# WASM First Five Minutes Implementation Plan

Status: Won't do

This plan is retained as historical context only.

The framework-wide WASM lane was cut before release. Autumn's first-five-minutes story is now server-first: route macros, Maud, htmx, Tailwind, static JavaScript, and raw Axum when a page grows into a client application.

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Autumn's WASM/islands story genuinely work end-to-end for a brand-new user: `autumn new --wasm`, `autumn setup --wasm`, `autumn dev`, one hydrated island with one typed server action, and `autumn build` producing deployable browser assets plus manifest.

**Architecture:** Ship an honest 0.2 WASM lane instead of the current aspirational one. The release path should use explicit, working pieces: `autumn_web::wasm::island(...)`, `autumn_web::wasm::assets()`, `#[server]`, a real `src/client.rs` browser bootstrap, and a CLI asset pipeline that post-processes wasm and writes `target/autumn/wasm/manifest.json`. Do **not** block 0.2 on a richer `IslandCx` DSL unless it is fully implemented; the current fake scaffold and unused `.islands(...)` path should not be the first-five-minutes story.

**Tech Stack:** `autumn-cli`, `autumn-web`, `autumn-wasm`, `wasm-bindgen`, `wasm-bindgen-cli-support`, `serde`, `axum`, `maud`, `tokio`

---

## Release Contract

Before any implementation, treat these as the 0.2 release gate:

1. `autumn new demo --wasm` generates a project that is truthful and runnable.
2. `autumn setup --wasm` validates the Rust target needed for the browser bundle.
3. `autumn dev` compiles both the server and the browser client when `src/client.rs` exists.
4. The generated app renders a server fallback, hydrates in the browser, and performs one typed `#[server]` round-trip.
5. `autumn build` emits browser assets plus a manifest that `autumn_web::wasm::assets()` can consume.
6. The generated page still has a usable non-WASM/htmx fallback.

---

### Task 1: Lock The 0.2 Contract In Tests

**Files:**
- Modify: `autumn-cli/src/new.rs`
- Modify: `autumn-cli/src/build.rs`
- Modify: `autumn-cli/src/dev.rs`
- Create: `autumn-cli/tests/wasm_first_five_minutes.rs`

**Step 1: Write the failing scaffold tests**

Add tests that assert a `--wasm` scaffold contains:

```rust
assert!(cargo.contains("[[bin]]"));
assert!(cargo.contains("path = \"src/client.rs\""));
assert!(cargo.contains("autumn-wasm"));
assert!(client.contains("#[wasm_bindgen(start)]"));
assert!(client.contains("autumn_wasm::boot"));
assert!(main.contains("autumn_web::wasm::assets()"));
assert!(main.contains(".actions("));
```

**Step 2: Run the targeted tests to verify they fail**

Run: `cargo test -p autumn-cli wasm`

Expected: FAIL because the current scaffold only type-checks a fake island and does not bootstrap a real browser entrypoint.

**Step 3: Write the failing build/dev pipeline tests**

Add unit tests for helper functions that will exist after the refactor:

```rust
assert_eq!(manifest.entry_js.as_deref(), Some("/static/autumn/client.js"));
assert_eq!(manifest.entry_wasm.as_deref(), Some("/static/autumn/client_bg.wasm"));
```

Also add one failing test that a wasm-enabled package should produce a manifest path under `target/autumn/wasm/manifest.json`.

**Step 4: Run the targeted tests to verify they fail**

Run: `cargo test -p autumn-cli wasm_first_five_minutes`

Expected: FAIL with missing helper / missing manifest assertions.

**Step 5: Commit**

```bash
git add autumn-cli/src/new.rs autumn-cli/src/build.rs autumn-cli/src/dev.rs autumn-cli/tests/wasm_first_five_minutes.rs
git commit -m "test: lock wasm first-five-minutes contract"
```

---

### Task 2: Replace The Fake `--wasm` Scaffold With A Real One

**Files:**
- Create: `autumn-cli/src/templates/main_wasm.rs.tmpl`
- Create: `autumn-cli/src/templates/lib_wasm.rs.tmpl`
- Create: `autumn-cli/src/templates/client_wasm.rs.tmpl`
- Modify: `autumn-cli/src/templates/Cargo.toml.tmpl`
- Modify: `autumn-cli/src/new.rs`
- Modify: `autumn-cli/src/templates/main.rs.tmpl`
- Modify: `autumn-cli/src/templates/lib.rs.tmpl`
- Modify: `autumn-cli/src/templates/client.rs.tmpl`

**Step 1: Write the failing test for template selection**

Add a test that a wasm project uses dedicated wasm templates, not the non-wasm `main.rs.tmpl` path.

```rust
let main = fs::read_to_string(tmp.path().join("wasm-app/src/main.rs")).unwrap();
assert!(main.contains("autumn_web::wasm::assets()"));
```

**Step 2: Run the test to verify it fails**

Run: `cargo test -p autumn-cli generated_wasm_project_uses_real_wasm_templates`

Expected: FAIL because the scaffold still renders the plain server template.

**Step 3: Implement a truthful generated app**

The generated wasm app should use:

```rust
// src/main.rs
#[get("/")]
async fn index() -> Markup {
    html! {
        head { (autumn_web::wasm::assets()) }
        body {
            (autumn_web::wasm::island(
                autumn_web::wasm::IslandMeta {
                    name: "counter",
                    mount_id: "counter",
                    props_type: "CounterProps",
                },
                CounterProps { initial: 3 },
                html! {
                    div id="counter" {
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
        .actions(actions![increment])
        .run()
        .await;
}
```

```rust
// src/lib.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterProps { pub initial: i32 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementInput { pub current: i32 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterView { pub value: i32 }

#[server]
pub async fn increment(input: IncrementInput) -> AutumnResult<CounterView> {
    Ok(CounterView { value: input.current + 1 })
}
```

```rust
// src/client.rs
#[wasm_bindgen(start)]
pub fn start() {
    autumn_wasm::boot(&[
        autumn_wasm::IslandRegistration::new("counter", "counter", mount_counter),
    ]);
}
```

Do **not** keep the fake `IslandCx` stub in the scaffold unless a real runtime type exists.

**Step 4: Run the scaffold tests**

Run: `cargo test -p autumn-cli generated_wasm_project`

Expected: PASS

**Step 5: Commit**

```bash
git add autumn-cli/src/templates autumn-cli/src/new.rs
git commit -m "feat: scaffold a real wasm starter app"
```

---

### Task 3: Add Minimal Browser Helpers Needed By The Honest Path

**Files:**
- Modify: `autumn-wasm/src/lib.rs`
- Modify: `autumn-wasm/src/island.rs`
- Modify: `autumn-wasm/src/boot.rs`
- Create: `autumn-wasm/src/dom.rs`
- Modify: `autumn-wasm/Cargo.toml`
- Test: `autumn-wasm/tests/wasm_smoke.rs`

**Step 1: Write the failing helper test**

Add a failing test for small, explicit helpers that the scaffold can use:

```rust
assert_eq!(decode_props::<CounterProps>("{\"initial\":3}").unwrap().initial, 3);
```

**Step 2: Run the test to verify it fails**

Run: `cargo test -p autumn-wasm`

Expected: FAIL because the helper does not exist yet.

**Step 3: Implement only the minimum helper surface**

Prefer a tiny explicit helper set over a fake reactive DSL. Good targets:

```rust
pub fn decode_props<T: serde::de::DeserializeOwned>(props_json: &str) -> Result<T, String> {
    serde_json::from_str(props_json).map_err(|error| error.to_string())
}
```

If the generated app needs tiny DOM helpers, add them explicitly (for example: query selector, set text, attach click callback). Do **not** invent `IslandCx::signal()` / `IslandCx::text()` unless you are prepared to support them as a public API.

**Step 4: Run tests**

Run: `cargo test -p autumn-wasm`

Expected: PASS

**Step 5: Commit**

```bash
git add autumn-wasm/src autumn-wasm/Cargo.toml autumn-wasm/tests/wasm_smoke.rs
git commit -m "feat: add minimal browser helpers for wasm starter path"
```

---

### Task 4: Implement The CLI WASM Asset Pipeline

**Files:**
- Modify: `autumn-cli/Cargo.toml`
- Create: `autumn-cli/src/wasm.rs`
- Modify: `autumn-cli/src/build.rs`
- Modify: `autumn-cli/src/dev.rs`
- Modify: `autumn-cli/src/main.rs`
- Test: `autumn-cli/tests/wasm_first_five_minutes.rs`

**Step 1: Write the failing asset-pipeline tests**

Add failing tests for helpers that:

1. locate the compiled `wasm32-unknown-unknown` artifact
2. run wasm-bindgen post-processing
3. fingerprint/copy `client.js` and `client_bg.wasm`
4. write `target/autumn/wasm/manifest.json`

Example assertions:

```rust
assert!(manifest_path.ends_with("target/autumn/wasm/manifest.json"));
assert!(entry_js.starts_with("/static/autumn/"));
assert!(entry_wasm.ends_with("_bg.wasm"));
```

**Step 2: Run the tests to verify they fail**

Run: `cargo test -p autumn-cli wasm_asset_pipeline`

Expected: FAIL because the CLI only runs `cargo build --target wasm32-unknown-unknown` and stops there.

**Step 3: Implement the real pipeline**

Use `wasm-bindgen-cli-support` inside the CLI instead of requiring the user to install a separate tool manually.

Add a helper module that:

```rust
pub fn build_browser_bundle(...) -> Result<WasmManifest, String>;
pub fn write_manifest(path: &Path, manifest: &WasmManifest) -> Result<(), String>;
```

Behavior:

1. compile `src/client.rs` for `wasm32-unknown-unknown`
2. post-process to browser JS + wasm glue
3. copy hashed assets into `static/autumn/` for dev
4. copy the same assets into `dist/static/autumn/` during `autumn build`
5. write `target/autumn/wasm/manifest.json`

**Step 4: Wire `autumn dev` and `autumn build` through the helper**

Keep the existing "detect `src/client.rs`" behavior, but make it produce usable assets instead of a raw `.wasm` object file.

**Step 5: Run the tests**

Run: `cargo test -p autumn-cli wasm`

Expected: PASS

**Step 6: Commit**

```bash
git add autumn-cli/Cargo.toml autumn-cli/src/main.rs autumn-cli/src/build.rs autumn-cli/src/dev.rs autumn-cli/src/wasm.rs autumn-cli/tests/wasm_first_five_minutes.rs
git commit -m "feat: build and manifest real wasm browser assets"
```

---

### Task 5: Make The First-Five-Minutes Story Honest In Runtime And Docs

**Files:**
- Modify: `autumn/src/wasm/mod.rs`
- Modify: `autumn/src/lib.rs`
- Modify: `README.md`
- Modify: `docs/guide/getting-started.md`
- Modify: `examples/reddit-clone/README.md`

**Step 1: Write the failing docs/runtime assertions**

Add or update tests that prove:

```rust
assert_eq!(assets().into_string(), "");
assert!(markup.contains("script type=\"module\""));
```

Also add a doc-review checklist for public WASM APIs:

- no docs claiming `IslandCx` exists if it does not
- no docs implying `.islands(...)` is required unless runtime consumes it
- scaffold instructions use the real working path

**Step 2: Run the tests / review pass**

Run: `cargo test -p autumn-web wasm`

Expected: existing tests pass, but docs still need correction.

**Step 3: Make the public story honest**

For 0.2:

1. document the working path: `wasm::assets()`, `wasm::island(...)`, `#[server]`, `src/client.rs`
2. remove fake `IslandCx` examples from first-five docs/templates
3. if `.islands(...)` remains unused, stop presenting it as part of the release path

If you keep `#[island]` and `.islands(...)` public, clearly mark them as non-release-path / experimental until they are backed by a real runtime contract.

**Step 4: Run the docs-targeted tests**

Run: `cargo test -p autumn-web`

Expected: PASS

**Step 5: Commit**

```bash
git add autumn/src/wasm/mod.rs autumn/src/lib.rs README.md docs/guide/getting-started.md examples/reddit-clone/README.md
git commit -m "docs: make wasm first-five-minutes story honest"
```

---

### Task 6: Add An End-To-End Release Gate

**Files:**
- Modify: `autumn-cli/tests/e2e.rs`
- Create: `autumn-cli/tests/e2e_wasm.rs`
- Optionally create: `examples/wasm-counter/Cargo.toml`
- Optionally create: `examples/wasm-counter/src/main.rs`
- Optionally create: `examples/wasm-counter/src/lib.rs`
- Optionally create: `examples/wasm-counter/src/client.rs`

**Step 1: Write the failing e2e test**

Prefer an ignored integration test that:

1. runs `autumn new demo --wasm`
2. patches path dependencies to local workspace crates
3. runs `cargo build`
4. runs `autumn build --debug`
5. asserts the generated project contains:
   - `target/autumn/wasm/manifest.json`
   - `static/autumn/` assets for dev
   - `dist/static/autumn/` assets for build
   - HTML containing the island mount plus `<script type="module" ...>`

**Step 2: Run the test to verify it fails**

Run: `cargo test -p autumn-cli --test e2e_wasm -- --ignored`

Expected: FAIL before the pipeline is complete.

**Step 3: Implement only enough support to make the test green**

Do not add a second fancy example until the generated project path works.

If a workspace fixture is easier to keep stable than the generated-project test, add `examples/wasm-counter` as a canonical smoke target and use it in CI.

**Step 4: Run the full verification set**

Run:

```bash
cargo test -p autumn-wasm
cargo test -p autumn-web
cargo test -p autumn-cli
cargo test -p autumn-cli --test e2e_wasm -- --ignored
```

Expected: PASS

**Step 5: Commit**

```bash
git add autumn-cli/tests/e2e.rs autumn-cli/tests/e2e_wasm.rs examples/wasm-counter
git commit -m "test: add end-to-end wasm release gate"
```

---

### Task 7: Retrofit `reddit-clone` After The Framework Path Is Green

**Files:**
- Modify: `examples/reddit-clone/Cargo.toml`
- Create: `examples/reddit-clone/src/client.rs`
- Modify: `examples/reddit-clone/src/main.rs`
- Modify: `examples/reddit-clone/src/islands.rs`
- Modify: `examples/reddit-clone/src/routes/layout.rs`
- Modify: `examples/reddit-clone/src/routes/posts.rs`
- Modify: `examples/reddit-clone/src/routes/subreddits.rs`

**Step 1: Write one failing integration-focused test**

Add a test that the example's rendered HTML contains the island marker and asset tags when the wasm manifest exists.

**Step 2: Run it to verify it fails**

Run: `cargo test -p reddit-clone wasm`

Expected: FAIL

**Step 3: Migrate `reddit-clone` to the real path**

Use the framework contract proven above:

1. real `src/client.rs`
2. `autumn_web::wasm::assets()` in the layout head
3. real vote island mount markup
4. typed server action or explicit wasm client call for votes
5. preserve the existing htmx fallback

Do **not** attempt this before Tasks 1-6 are green, or you will end up debugging the example instead of the framework.

**Step 4: Verify**

Run: `cargo test -p reddit-clone`

Expected: PASS

**Step 5: Commit**

```bash
git add examples/reddit-clone
git commit -m "feat: migrate reddit-clone to real wasm island pipeline"
```

---

## Notes

- The current public WASM story overclaims. The release fix is partly implementation and partly honesty.
- `.actions(...)` is real because it mounts routes. `.islands(...)` is currently bookkeeping only. Do not treat those as equivalent.
- The generated app should be the primary release gate. `reddit-clone` is a secondary proof, not the source of truth.
- If time runs short, cut fake ergonomics before cutting correctness.
