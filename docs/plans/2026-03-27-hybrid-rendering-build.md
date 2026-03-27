# Hybrid Rendering Phase 2: `autumn build` Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the `autumn build` CLI command that pre-renders `#[static_get]` routes to HTML files, producing a `dist/` directory with a `manifest.json` the runtime already knows how to serve.

**Architecture:** The build is a two-process design. The CLI (`autumn build`) compiles the user's binary in release mode, then executes it with `AUTUMN_BUILD_STATIC=1`. The runtime library detects this env var in `AppBuilder::run()`, enters "build mode" instead of serving — it renders all static routes via the Axum router, writes HTML to a staging dir, then atomically renames to `dist/`. The CLI orchestrates, the runtime renders.

**Tech Stack:** Rust, Clap (CLI), Tokio (async rendering), Axum (oneshot requests), tower::ServiceExt (handler invocation), serde_json (manifest)

---

## Architecture Overview

```
autumn build (CLI)
  │
  ├─ 1. cargo build --release
  │
  └─ 2. Run binary with AUTUMN_BUILD_STATIC=1
         │
         AppBuilder::run() detects env var
         │
         ├─ Build Axum router (same as production)
         ├─ Collect StaticRouteMeta list
         ├─ For each meta:
         │    ├─ Send oneshot GET request through router
         │    ├─ Validate response status (must be 2xx)
         │    └─ Write body to staging_dir/{url_to_file_path}
         ├─ Write manifest.json to staging_dir
         └─ Atomic rename staging_dir → dist/
```

Key files involved:
- `autumn-cli/src/main.rs` — Add `Build` command variant
- `autumn-cli/src/build.rs` — CLI orchestration (cargo build + run binary)
- `autumn/src/static_gen/mod.rs` — Add `build` submodule export
- `autumn/src/static_gen/build.rs` — **NEW**: Core render engine
- `autumn/src/app.rs` — Add `static_routes()` method + build mode detection
- `autumn-macros/src/static_routes_macro.rs` — **NEW**: `static_routes![]` collector macro
- `autumn-macros/src/lib.rs` — Export `static_routes` proc macro
- `autumn/src/lib.rs` — Re-export `static_routes` macro + prelude
- `examples/blog/src/main.rs` — Wire up `.static_routes()`

---

## Task 1: `static_routes![]` Collection Macro

The build needs to know which handlers are static. Just like `routes![]` calls `__autumn_route_info_*`, we need `static_routes![]` to call `__autumn_static_meta_*`.

**Files:**
- Create: `autumn-macros/src/static_routes_macro.rs`
- Modify: `autumn-macros/src/lib.rs` — add `mod static_routes_macro` + proc macro entry
- Modify: `autumn/src/lib.rs` — re-export + prelude
- Test: `autumn-macros/src/static_routes_macro.rs` (unit tests in the macro file)

**Step 1: Write the macro**

Create `autumn-macros/src/static_routes_macro.rs`:

```rust
//! `static_routes![]` collection macro.
//!
//! Collects `#[static_get]`-annotated handlers into a `Vec<StaticRouteMeta>`
//! by calling their `__autumn_static_meta_{name}()` companions.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Path, Token, punctuated::Punctuated};

pub fn static_routes_macro(input: TokenStream) -> TokenStream {
    if input.is_empty() {
        return quote! { ::std::vec::Vec::new() };
    }

    let paths: Punctuated<Path, Token![,]> =
        match syn::parse::Parser::parse2(Punctuated::parse_terminated, input) {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error(),
        };

    let meta_calls: Vec<_> = paths
        .iter()
        .map(|path| {
            let mut meta_path = path.clone();
            if let Some(last) = meta_path.segments.last_mut() {
                let meta_name = format_ident!("__autumn_static_meta_{}", last.ident);
                last.ident = meta_name;
            }
            quote! { #meta_path() }
        })
        .collect();

    quote! {
        vec![#(#meta_calls),*]
    }
}
```

**Step 2: Wire into lib.rs and prelude**

In `autumn-macros/src/lib.rs`, add:
```rust
mod static_routes_macro;

/// Collect `#[static_get]` handlers into a `Vec<StaticRouteMeta>`.
#[proc_macro]
pub fn static_routes(input: TokenStream) -> TokenStream {
    static_routes_macro::static_routes_macro(input.into()).into()
}
```

In `autumn/src/lib.rs`, add re-export:
```rust
pub use autumn_macros::static_routes;
```

In `autumn/src/prelude.rs`, add `static_routes` to the route macros use line.

**Step 3: Write unit test for parse**

Add to `autumn-macros/src/lib.rs` tests section a parse test for the new subcommand.

**Step 4: Run tests**

```bash
cargo test -p autumn-macros
```
Expected: all pass (macro compiles, no runtime test yet since we can't invoke proc macros in unit tests — validation comes from compile-pass test in Task 2)

**Step 5: Write compile-pass test**

Create `autumn/tests/compile-pass/static_routes_basic.rs`:
```rust
use autumn_web::prelude::*;

#[static_get("/about")]
async fn about() -> &'static str { "About" }

fn main() {
    let metas: Vec<autumn_web::static_gen::StaticRouteMeta> =
        static_routes![about];
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].path, "/about");
}
```

Add to `autumn/tests/compile_fail.rs` in `compile_pass_tests()`:
```rust
t.pass("tests/compile-pass/static_routes_basic.rs");
```

**Step 6: Run all tests**

```bash
cargo test -p autumn-web
```
Expected: all pass including new compile-pass test

**Step 7: Commit**

```bash
git add autumn-macros/src/static_routes_macro.rs autumn-macros/src/lib.rs \
        autumn/src/lib.rs autumn/src/prelude.rs \
        autumn/tests/compile-pass/static_routes_basic.rs autumn/tests/compile_fail.rs
git commit -m "feat: add static_routes![] collection macro"
```

---

## Task 2: `AppBuilder::static_routes()` Method

`AppBuilder` needs to accept static route metadata, just like it accepts routes and tasks.

**Files:**
- Modify: `autumn/src/app.rs` — add `static_metas: Vec<StaticRouteMeta>` field + `.static_routes()` method

**Step 1: Write test for the builder method**

In `autumn/src/app.rs` tests, add:
```rust
#[test]
fn app_builder_accepts_static_routes() {
    use crate::static_gen::StaticRouteMeta;
    let metas = vec![StaticRouteMeta {
        path: "/about",
        name: "about",
        revalidate: None,
    }];
    let builder = app().static_routes(metas);
    // Builder compiles and holds the metas — that's the assertion
    assert_eq!(builder.static_metas.len(), 1);
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p autumn-web app_builder_accepts_static_routes
```
Expected: FAIL — `static_metas` field and `static_routes()` method don't exist

**Step 3: Implement**

In `AppBuilder` struct, add:
```rust
pub struct AppBuilder {
    routes: Vec<Route>,
    tasks: Vec<crate::task::TaskInfo>,
    static_metas: Vec<crate::static_gen::StaticRouteMeta>,
}
```

Update `app()` constructor:
```rust
pub const fn app() -> AppBuilder {
    AppBuilder {
        routes: Vec::new(),
        tasks: Vec::new(),
        static_metas: Vec::new(),
    }
}
```

Add the method:
```rust
/// Register static route metadata for build-time rendering.
///
/// Use the [`static_routes!`](crate::static_routes) macro to collect
/// `#[static_get]` handlers' metadata.
#[must_use]
pub fn static_routes(
    mut self,
    metas: Vec<crate::static_gen::StaticRouteMeta>,
) -> Self {
    self.static_metas.extend(metas);
    self
}
```

**Step 4: Run test**

```bash
cargo test -p autumn-web app_builder_accepts_static_routes
```
Expected: PASS

**Step 5: Commit**

```bash
git add autumn/src/app.rs
git commit -m "feat: add AppBuilder::static_routes() method"
```

---

## Task 3: Static Build Renderer

The core rendering engine. Takes a built Axum router + list of `StaticRouteMeta`, sends oneshot requests through the router, writes HTML to disk with atomic swap.

**Files:**
- Create: `autumn/src/static_gen/build.rs`
- Modify: `autumn/src/static_gen/mod.rs` — add `pub mod build`

**Step 1: Write tests first**

Create `autumn/src/static_gen/build.rs` with tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_gen::StaticRouteMeta;

    fn test_meta(path: &'static str, name: &'static str) -> StaticRouteMeta {
        StaticRouteMeta { path, name, revalidate: None }
    }

    /// Router that returns "Hello from {path}" for any GET request.
    fn echo_router() -> axum::Router {
        axum::Router::new()
            .fallback(axum::routing::get(|uri: axum::http::Uri| async move {
                format!("Hello from {}", uri.path())
            }))
    }

    #[tokio::test]
    async fn renders_single_route_to_dist() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");

        let result = render_static_routes(
            echo_router(),
            &[test_meta("/about", "about")],
            &dist,
        ).await;

        assert!(result.is_ok(), "render failed: {:?}", result.err());
        // Check file exists
        let html = std::fs::read_to_string(dist.join("about/index.html")).unwrap();
        assert_eq!(html, "Hello from /about");
        // Check manifest
        let manifest = StaticManifest::load(&dist.join("manifest.json")).unwrap();
        assert_eq!(manifest.routes.len(), 1);
        assert!(manifest.routes.contains_key("/about"));
    }

    #[tokio::test]
    async fn renders_root_route() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");

        let result = render_static_routes(
            echo_router(),
            &[test_meta("/", "index")],
            &dist,
        ).await;

        assert!(result.is_ok());
        let html = std::fs::read_to_string(dist.join("index.html")).unwrap();
        assert_eq!(html, "Hello from /");
    }

    #[tokio::test]
    async fn rejects_non_2xx_response() {
        let router = axum::Router::new()
            .fallback(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") });

        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");

        let result = render_static_routes(
            router,
            &[test_meta("/about", "about")],
            &dist,
        ).await;

        assert!(result.is_err());
        // dist/ should NOT exist (atomic swap didn't happen)
        assert!(!dist.exists());
    }

    #[tokio::test]
    async fn cleans_stale_dist_before_build() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        // Pre-create a stale file
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("stale.html"), "old").unwrap();

        let result = render_static_routes(
            echo_router(),
            &[test_meta("/about", "about")],
            &dist,
        ).await;

        assert!(result.is_ok());
        // Stale file should be gone
        assert!(!dist.join("stale.html").exists());
        // New file should exist
        assert!(dist.join("about/index.html").exists());
    }

    #[tokio::test]
    async fn renders_multiple_routes() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");

        let result = render_static_routes(
            echo_router(),
            &[
                test_meta("/", "index"),
                test_meta("/about", "about"),
                test_meta("/contact", "contact"),
            ],
            &dist,
        ).await;

        assert!(result.is_ok());
        let manifest = StaticManifest::load(&dist.join("manifest.json")).unwrap();
        assert_eq!(manifest.routes.len(), 3);
    }
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p autumn-web static_gen::build
```
Expected: FAIL — `render_static_routes` doesn't exist

**Step 3: Implement `render_static_routes`**

```rust
//! Static build renderer.
//!
//! Renders `#[static_get]` routes through the Axum router and writes
//! the output HTML to a staging directory, then atomically swaps to
//! `dist/`. This is the engine behind `autumn build`.

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::{ManifestEntry, StaticManifest, StaticRouteMeta, url_to_file_path};

/// Errors that can occur during static rendering.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("Route {path} returned HTTP {status} (expected 2xx)")]
    NonSuccessStatus { path: String, status: StatusCode },

    #[error("Failed to read response body for {path}: {source}")]
    BodyRead { path: String, source: axum::Error },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Render all static routes and write them to `dist_dir`.
///
/// 1. Renders to a staging directory (`{dist_dir}.staging`).
/// 2. On success, atomically renames staging → dist.
/// 3. On failure, removes staging and returns error.
///
/// If `dist_dir` already exists, it is replaced.
pub async fn render_static_routes(
    router: axum::Router,
    metas: &[StaticRouteMeta],
    dist_dir: &Path,
) -> Result<(), BuildError> {
    let staging = dist_dir.with_extension("staging");

    // Clean staging dir if it exists from a previous failed build
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    let mut manifest_routes = std::collections::HashMap::new();

    for meta in metas {
        eprintln!("  Rendering {} ...", meta.path);

        // Send a oneshot GET request through the full router
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(meta.path)
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router should not error");

        // Validate status code
        if !response.status().is_success() {
            // Clean up staging
            let _ = std::fs::remove_dir_all(&staging);
            return Err(BuildError::NonSuccessStatus {
                path: meta.path.to_owned(),
                status: response.status(),
            });
        }

        // Read body
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map_err(|e| BuildError::BodyRead {
                path: meta.path.to_owned(),
                source: e,
            })?;

        // Write to file
        let file_path = url_to_file_path(meta.path);
        let full_path = staging.join(&file_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, &body_bytes)?;

        manifest_routes.insert(
            meta.path.to_owned(),
            ManifestEntry {
                file: file_path,
                revalidate: meta.revalidate,
            },
        );
    }

    // Write manifest
    let manifest = StaticManifest {
        generated_at: chrono_or_manual_timestamp(),
        autumn_version: env!("CARGO_PKG_VERSION").to_owned(),
        routes: manifest_routes,
    };
    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(staging.join("manifest.json"), json)?;

    // Atomic swap: remove old dist, rename staging → dist
    if dist_dir.exists() {
        std::fs::remove_dir_all(dist_dir)?;
    }
    std::fs::rename(&staging, dist_dir)?;

    Ok(())
}

/// Simple ISO-8601-ish timestamp without pulling in chrono.
fn chrono_or_manual_timestamp() -> String {
    // Use humantime for formatting or just a simple approach
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Format as Unix timestamp — good enough for manifest purposes.
    // A human-readable format would need the `time` or `chrono` crate.
    format!("{}", duration.as_secs())
}
```

**Step 4: Update `static_gen/mod.rs`**

```rust
pub mod build;
mod middleware;
mod types;

pub use build::{render_static_routes, BuildError};
pub use middleware::StaticFileLayer;
pub use types::{ManifestEntry, StaticManifest, StaticRouteMeta, url_to_file_path};
```

**Step 5: Add `serde_json` as a non-dev dependency**

In `autumn/Cargo.toml`, ensure `serde_json` is in `[dependencies]` (not just `[dev-dependencies]`), since the build module uses it at runtime.

**Step 6: Run tests**

```bash
cargo test -p autumn-web static_gen::build
```
Expected: all 5 tests pass

**Step 7: Commit**

```bash
git add autumn/src/static_gen/build.rs autumn/src/static_gen/mod.rs autumn/Cargo.toml
git commit -m "feat: add static build renderer engine"
```

---

## Task 4: Build Mode Detection in `AppBuilder::run()`

When `AUTUMN_BUILD_STATIC=1` is set, `AppBuilder::run()` should render static routes instead of starting the HTTP server.

**Files:**
- Modify: `autumn/src/app.rs` — add build mode branch in `run()`

**Step 1: Write integration test**

Create `autumn/tests/static_build_mode.rs`:

```rust
//! Tests that the build renderer produces correct output end-to-end
//! when invoked through the static_gen::build API.
//!
//! We don't test the full AppBuilder::run() path here (that requires
//! env var coordination). Instead we test render_static_routes directly
//! with a realistic router setup.

use autumn_web::app::build_router;
use autumn_web::config::AutumnConfig;
use autumn_web::route::Route;
use autumn_web::static_gen::{StaticRouteMeta, render_static_routes};

fn about_route() -> Route {
    Route {
        method: http::Method::GET,
        path: "/about",
        handler: axum::routing::get(|| async { "About Page Content" }),
        name: "about",
    }
}

fn about_meta() -> StaticRouteMeta {
    StaticRouteMeta {
        path: "/about",
        name: "about",
        revalidate: None,
    }
}

fn test_state() -> autumn_web::AppState {
    autumn_web::AppState {
        #[cfg(feature = "db")]
        pool: None,
        profile: None,
        started_at: std::time::Instant::now(),
        health_detailed: false,
    }
}

#[tokio::test]
async fn build_mode_renders_through_real_router() {
    let config = AutumnConfig::default();
    let router = build_router(vec![about_route()], &config, test_state());

    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("dist");

    let result = render_static_routes(router, &[about_meta()], &dist).await;
    assert!(result.is_ok(), "build failed: {:?}", result.err());

    let html = std::fs::read_to_string(dist.join("about/index.html")).unwrap();
    assert_eq!(html, "About Page Content");
}
```

**Step 2: Run test**

```bash
cargo test -p autumn-web --test static_build_mode
```
Expected: PASS (uses already-implemented `render_static_routes`)

**Step 3: Add build mode to `AppBuilder::run()`**

In `app.rs`, at the start of `run()`, add:

```rust
pub async fn run(self) {
    // ── Build mode ─────────────────────────────────────────────────
    // When AUTUMN_BUILD_STATIC=1, render static routes to dist/ and exit.
    if std::env::var("AUTUMN_BUILD_STATIC").as_deref() == Ok("1") {
        self.run_build_mode().await;
        return;
    }

    // ... existing server startup code ...
}

/// Render all registered static routes to `dist/` and exit.
async fn run_build_mode(self) {
    let config = AutumnConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load configuration: {e}");
        std::process::exit(1);
    });

    crate::logging::init(&config.log);

    if self.static_metas.is_empty() {
        eprintln!("No static routes registered. Nothing to build.");
        eprintln!("Hint: use .static_routes(static_routes![...]) on your AppBuilder.");
        std::process::exit(1);
    }

    let state = AppState {
        #[cfg(feature = "db")]
        pool: {
            if let Some(ref url) = config.database.url {
                Some(db::create_pool(url, &config.database).expect("database pool"))
            } else {
                None
            }
        },
        profile: None,
        started_at: std::time::Instant::now(),
        health_detailed: false,
    };

    let router = build_router(self.routes, &config, state);

    let dist_dir = std::env::var("AUTUMN_MANIFEST_DIR").map_or_else(
        |_| std::path::PathBuf::from("dist"),
        |d| std::path::PathBuf::from(d).join("dist"),
    );

    eprintln!("Building {} static route(s)...", self.static_metas.len());
    match crate::static_gen::render_static_routes(router, &self.static_metas, &dist_dir).await {
        Ok(()) => {
            eprintln!("✓ Static build complete → {}", dist_dir.display());
        }
        Err(e) => {
            eprintln!("✗ Static build failed: {e}");
            std::process::exit(1);
        }
    }
}
```

**Step 4: Run all tests**

```bash
cargo test -p autumn-web
```
Expected: all pass

**Step 5: Commit**

```bash
git add autumn/src/app.rs autumn/tests/static_build_mode.rs
git commit -m "feat: add build mode to AppBuilder (AUTUMN_BUILD_STATIC=1)"
```

---

## Task 5: `autumn build` CLI Command

The CLI command that orchestrates: `cargo build --release` then runs the binary with `AUTUMN_BUILD_STATIC=1`.

**Files:**
- Create: `autumn-cli/src/build.rs`
- Modify: `autumn-cli/src/main.rs` — add `Build` variant

**Step 1: Add the Build command to Clap**

In `autumn-cli/src/main.rs`:

```rust
mod build;

enum Commands {
    // ... existing ...

    /// Pre-render static routes to dist/
    Build {
        /// Build in debug mode instead of release
        #[arg(long)]
        debug: bool,
    },
}

// In match:
Commands::Build { debug } => build::run(debug),
```

**Step 2: Implement `build::run()`**

Create `autumn-cli/src/build.rs`:

```rust
//! `autumn build` — compile the app and pre-render static routes.

use std::process::Command;

/// Run the static build pipeline.
///
/// 1. Compile the user's binary (release by default).
/// 2. Execute the binary with `AUTUMN_BUILD_STATIC=1`.
///
/// The binary's `AppBuilder::run()` detects the env var and renders
/// static routes instead of starting the server.
pub fn run(debug: bool) {
    eprintln!("🍂 autumn build\n");

    // Step 1: Compile
    let profile = if debug { "dev" } else { "release" };
    let mut cargo = Command::new("cargo");
    cargo.arg("build");
    if !debug {
        cargo.arg("--release");
    }

    eprintln!("Compiling ({profile} profile)...");
    let status = cargo.status().expect("failed to run cargo build");
    if !status.success() {
        eprintln!("✗ Compilation failed");
        std::process::exit(1);
    }

    // Step 2: Find the binary
    // Use cargo metadata to find the binary name and target dir
    let binary = find_binary(debug);
    eprintln!("Running static renderer...\n");

    // Step 3: Run with AUTUMN_BUILD_STATIC=1
    let status = Command::new(&binary)
        .env("AUTUMN_BUILD_STATIC", "1")
        .status()
        .unwrap_or_else(|e| {
            eprintln!("✗ Failed to run {}: {e}", binary.display());
            std::process::exit(1);
        });

    if !status.success() {
        eprintln!("\n✗ Static build failed");
        std::process::exit(1);
    }

    eprintln!("\n🍂 Build complete!");
}

/// Locate the compiled binary.
///
/// Uses `cargo metadata` to find the workspace target directory and
/// binary name from the current package's `[[bin]]` targets.
fn find_binary(debug: bool) -> std::path::PathBuf {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .output()
        .expect("failed to run cargo metadata");

    if !output.status.success() {
        eprintln!("✗ Failed to read cargo metadata");
        std::process::exit(1);
    }

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata");

    let target_dir = metadata["target_directory"]
        .as_str()
        .expect("target_directory in metadata");

    // Find the current package's binary name.
    // Look for the "resolve" root, or fall back to the first package with a bin target.
    let packages = metadata["packages"].as_array().expect("packages array");

    // Find the "current" package: the one whose manifest_path is in the current directory
    let cwd = std::env::current_dir().expect("current dir");
    let bin_name = packages
        .iter()
        .filter(|pkg| {
            let manifest = pkg["manifest_path"].as_str().unwrap_or("");
            std::path::Path::new(manifest)
                .parent()
                .map_or(false, |dir| dir.starts_with(&cwd))
        })
        .flat_map(|pkg| {
            pkg["targets"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .filter(|t| {
                    t["kind"]
                        .as_array()
                        .map_or(false, |kinds| kinds.iter().any(|k| k == "bin"))
                })
                .filter_map(|t| t["name"].as_str().map(String::from))
        })
        .next()
        .unwrap_or_else(|| {
            eprintln!("✗ No binary target found in current package");
            std::process::exit(1);
        });

    let profile_dir = if debug { "debug" } else { "release" };
    let mut path = std::path::PathBuf::from(target_dir);
    path.push(profile_dir);
    path.push(&bin_name);

    if cfg!(target_os = "windows") {
        path.set_extension("exe");
    }

    if !path.exists() {
        eprintln!("✗ Binary not found at {}", path.display());
        eprintln!("  Expected after `cargo build --{profile_dir}`");
        std::process::exit(1);
    }

    path
}
```

**Step 3: Add serde_json to autumn-cli deps**

In `autumn-cli/Cargo.toml`, add:
```toml
serde_json = "1"
```

**Step 4: Write CLI parse tests**

In `autumn-cli/src/main.rs` tests:
```rust
#[test]
fn parse_build_subcommand() {
    let cli = Cli::try_parse_from(["autumn", "build"]).unwrap();
    assert!(matches!(cli.command, Commands::Build { debug: false }));
}

#[test]
fn parse_build_debug() {
    let cli = Cli::try_parse_from(["autumn", "build", "--debug"]).unwrap();
    assert!(matches!(cli.command, Commands::Build { debug: true }));
}
```

**Step 5: Run tests**

```bash
cargo test -p autumn-cli
```
Expected: parse tests pass

**Step 6: Commit**

```bash
git add autumn-cli/src/build.rs autumn-cli/src/main.rs autumn-cli/Cargo.toml
git commit -m "feat: add 'autumn build' CLI command"
```

---

## Task 6: Wire Up Blog Example

Update the blog example to use `.static_routes()` so `autumn build` works end-to-end.

**Files:**
- Modify: `examples/blog/src/main.rs` — add `.static_routes(static_routes![...])`

**Step 1: Update blog main.rs**

```rust
use autumn_web::{routes, static_routes};

// In the builder chain:
autumn_web::app()
    .routes(routes![...existing...])
    .static_routes(static_routes![
        routes::about::about,
    ])
    .run()
    .await;
```

**Step 2: Verify blog compiles**

```bash
cargo build -p blog
```
Expected: compiles

**Step 3: Test build mode manually**

```bash
cd examples/blog
AUTUMN_BUILD_STATIC=1 cargo run
```
Expected: renders `/about` to `dist/about/index.html` and prints success message.
(Note: this will skip database initialization if no DB is configured, and the about page doesn't need a DB.)

**Step 4: Commit**

```bash
git add examples/blog/src/main.rs
git commit -m "feat: wire blog example with static_routes for autumn build"
```

---

## Task 7: Full Test Suite & Lint Pass

Final verification that everything works together.

**Step 1: Run full workspace tests**

```bash
cargo test --workspace
```

**Step 2: Run clippy**

```bash
cargo clippy --workspace -- -D warnings
```

**Step 3: Run fmt**

```bash
cargo fmt --all --check
```

**Step 4: Fix any issues found**

**Step 5: Final commit (if any fixes)**

```bash
git commit -m "chore: fix lint and test issues from build feature"
```

---

## Summary

| Task | Description | Files | Est. Lines |
|------|-------------|-------|-----------|
| 1 | `static_routes![]` collection macro | 3 new, 3 modified | ~50 |
| 2 | `AppBuilder::static_routes()` method | 1 modified | ~25 |
| 3 | Static build renderer engine | 1 new, 1 modified | ~200 |
| 4 | Build mode detection in `run()` | 1 modified, 1 new test | ~60 |
| 5 | `autumn build` CLI command | 1 new, 2 modified | ~150 |
| 6 | Blog example wiring | 1 modified | ~5 |
| 7 | Full verification pass | 0 | 0 |

**Total:** ~490 lines of new code + tests

**Key design decisions from brainstorming doc applied:**
- ✅ Atomic build output (staging dir → rename)
- ✅ Clean build by default (staging is fresh)
- ✅ HTTP status validation (non-2xx = build failure)
- ✅ `tower-http::ServeDir` for serving (already done in Phase 1)
- ⏩ Parallel rendering — deferred to follow-up (sequential is correct first)
- ⏩ Startup route reconciliation — deferred (nice-to-have)
