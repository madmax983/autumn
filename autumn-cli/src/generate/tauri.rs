//! `autumn generate tauri` — scaffold a Tauri desktop wrapper around an autumn app.
//!
//! Creates a `src-tauri/` sub-project using the **sidecar model**: the autumn
//! server binary is supervised by the Tauri shell, and the webview loads the app
//! from a loopback port chosen at runtime (no hardcoded port, no orphaned process).
//! The entire existing autumn app — routes, Maud/htmx, sessions — runs unmodified;
//! the generator does **not** rewrite handlers or move to a static export.
//!
//! # Runtime dependencies on autumn features
//!
//! The staging scripts build the sidecar with two autumn features that make the
//! packaged desktop app fully self-contained:
//!
//! - **#1119 managed local Postgres** (`autumn-web/managed-pg-bundled`) — embeds the
//!   Postgres binaries so the desktop app needs no separately-installed database.
//!   The sidecar is pointed at a per-app data directory via `AUTUMN_MANAGED_PG_DATA_DIR`.
//! - **#1004 single-binary asset embed** (`autumn-web/embed-assets`) — embeds the
//!   `static/` tree into the release binary so the packaged app has no loose files.
//!
//! # Generated files
//!
//! ```text
//! src-tauri/
//!   tauri.conf.json          — Tauri v2 config (productName, bundle, sidecar ref)
//!   Cargo.toml               — standalone shell crate with its own [workspace]
//!   build.rs                 — calls tauri_build::build()
//!   src/main.rs              — #![windows_subsystem] + calls lib::run()
//!   src/lib.rs               — sidecar lifecycle: bind loopback:0, spawn sidecar,
//!                               poll /health, open webview window, kill on exit
//!   icons/icon.svg           — SVG source (reuse PWA icon if present; replace to customise)
//!   icons/32x32.png          }
//!   icons/128x128.png        } placeholder icons so `cargo tauri build` succeeds
//!   icons/128x128@2x.png     } out-of-the-box; regenerate with `cargo tauri icon`
//!   icons/icon.png           }
//!   icons/icon.ico           — Windows
//!   icons/icon.icns          — macOS
//!   stage-sidecar.sh         — Unix: build sidecar → copy to binaries/
//!   stage-sidecar.ps1        — Windows: same in PowerShell
//!   .gitignore               — /target /binaries /gen
//! ```

use std::path::Path;

use super::emit::Plan;
use super::{Flags, GenerateError, ensure_project_root};

// ── Public API ────────────────────────────────────────────────────────────────

/// Compute the file actions for `autumn generate tauri`.
///
/// # Errors
/// Returns [`GenerateError::NotInProject`] when not at a project root, or
/// [`GenerateError::Config`] if `Cargo.toml` is missing `[package].name`.
pub fn plan_tauri(project_root: &Path) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;

    let package_name = read_package_name(project_root)?;
    let mut plan = Plan::new(project_root);
    let tauri = project_root.join("src-tauri");

    // Core Tauri project files
    plan.create(
        tauri.join("tauri.conf.json"),
        render_tauri_conf(&package_name),
    );
    plan.create(
        tauri.join("Cargo.toml"),
        render_shell_cargo_toml(&package_name),
    );
    plan.create(tauri.join("build.rs"), render_build_rs());
    plan.create(
        tauri.join("src").join("main.rs"),
        render_shell_main_rs(&package_name),
    );
    plan.create(
        tauri.join("src").join("lib.rs"),
        render_shell_lib_rs(&package_name),
    );

    // Icons — reuse the PWA icon when the user already ran `autumn generate pwa`
    let icons_dir = tauri.join("icons");
    let pwa_icon_src = project_root.join("static").join("icons").join("icon.svg");
    if pwa_icon_src.is_file() {
        let contents = std::fs::read_to_string(&pwa_icon_src).map_err(GenerateError::Io)?;
        plan.create_if_absent(icons_dir.join("icon.svg"), contents);
    } else {
        plan.create_if_absent(icons_dir.join("icon.svg"), render_placeholder_icon_svg());
    }
    // Placeholder raster icons so `cargo tauri build` works immediately.
    // Replace with proper icons by running: cargo tauri icon static/icons/icon.svg
    plan.create_bytes(icons_dir.join("32x32.png"), PLACEHOLDER_PNG);
    plan.create_bytes(icons_dir.join("128x128.png"), PLACEHOLDER_PNG);
    plan.create_bytes(icons_dir.join("128x128@2x.png"), PLACEHOLDER_PNG);
    plan.create_bytes(icons_dir.join("icon.png"), PLACEHOLDER_PNG);
    plan.create_bytes(icons_dir.join("icon.ico"), PLACEHOLDER_ICO);
    plan.create_bytes(icons_dir.join("icon.icns"), PLACEHOLDER_ICNS);

    // Staging scripts
    plan.create(
        tauri.join("stage-sidecar.sh"),
        render_stage_sidecar_sh(&package_name),
    );
    plan.create(
        tauri.join("stage-sidecar.ps1"),
        render_stage_sidecar_ps1(&package_name),
    );
    plan.create(tauri.join(".gitignore"), render_gitignore());

    Ok(plan)
}

/// CLI entry point — executes the plan and prints required prerequisites.
pub fn run(flags: Flags) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    match plan_tauri(&cwd).and_then(|p| p.execute(flags)) {
        Ok(()) => {
            if !flags.dry_run {
                println!("\n{}", render_prerequisites());
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ── Package name helper ───────────────────────────────────────────────────────

fn read_package_name(project_root: &Path) -> Result<String, GenerateError> {
    let cargo_path = project_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_path).map_err(GenerateError::Io)?;
    let doc: toml::Value = toml::from_str(&content)
        .map_err(|e| GenerateError::Config(format!("failed to parse Cargo.toml: {e}")))?;
    doc.get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_owned)
        .ok_or_else(|| GenerateError::Config("Cargo.toml missing [package].name".to_owned()))
}

// ── Content renderers ─────────────────────────────────────────────────────────

fn render_tauri_conf(package_name: &str) -> String {
    // Bundle identifier: reverse-DNS with underscores replaced by hyphens.
    // Apple's spec allows only alphanumerics, hyphens, and periods — underscores are invalid.
    let identifier = format!("com.example.{}", package_name.replace('_', "-"));
    // Display title: capitalise first letter of each word; split on both '-' and '_'
    // so kebab-case (`my-app` → `My App`) and snake_case (`my_app` → `My App`) both work.
    let title: String = package_name
        .split(['-', '_'])
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    // beforeBuildCommand must use the native shell for the host OS.
    // cfg!(windows) is evaluated when the generator binary compiles, which runs on
    // the same host where `cargo tauri build` will later be invoked.
    // Use `bash` explicitly — the staging script uses BASH_SOURCE and `pipefail`,
    // which are not supported by POSIX `sh` (e.g. dash on Debian/Ubuntu).
    let before_build_cmd = if cfg!(windows) {
        "powershell -ExecutionPolicy Bypass -File stage-sidecar.ps1"
    } else {
        "bash stage-sidecar.sh"
    };
    // Profile config entries always point to src-tauri/configs/, which the staging
    // script (beforeBuildCommand) populates at build time — copying real files or
    // creating empty stubs for profiles that don't yet exist.  An empty TOML file
    // is valid and results in no overrides (AutumnConfig treats it as a no-op).
    // This keeps the resource list in sync regardless of when profile files are created,
    // avoiding the silent loss of production settings when autumn-prod.toml is added
    // after `autumn generate tauri` was run.
    format!(
        r#"{{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "{title}",
  "version": "0.1.0",
  "identifier": "{identifier}",
  "build": {{
    "beforeBuildCommand": "{before_build_cmd}"
  }},
  "bundle": {{
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.png",
      "icons/icon.ico",
      "icons/icon.icns"
    ],
    "externalBin": [
      "binaries/{package_name}"
    ],
    "resources": {{
      "../autumn.toml": "autumn.toml",
      "configs/autumn-prod.toml": "autumn-prod.toml",
      "configs/autumn-production.toml": "autumn-production.toml",
      "configs/autumn-staging.toml": "autumn-staging.toml",
      "configs/autumn-development.toml": "autumn-development.toml",
      "configs/autumn-test.toml": "autumn-test.toml"
    }}
  }},
  "app": {{
    "security": {{
      "csp": null
    }}
  }}
}}
"#
    )
}

fn render_shell_cargo_toml(package_name: &str) -> String {
    let desktop_name = format!("{package_name}-desktop");
    format!(
        r#"[package]
name = "{desktop_name}"
version = "0.0.1"
edition = "2021"

# Standalone workspace so this crate is independent from the autumn app workspace —
# no change to the root Cargo.toml is needed.
[workspace]

[build-dependencies]
tauri-build = {{ version = "2", features = [] }}

[dependencies]
tauri = {{ version = "2", features = [] }}
tauri-plugin-shell = "2"

[profile.release]
panic = "abort"
codegen-units = 1
lto = true
opt-level = "s"
strip = true
"#
    )
}

fn render_build_rs() -> String {
    "fn main() {\n    tauri_build::build()\n}\n".to_owned()
}

fn render_shell_main_rs(package_name: &str) -> String {
    let lib_name = package_name.replace('-', "_") + "_desktop";
    format!(
        "#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]\n\
         \n\
         fn main() {{\n\
         \x20   {lib_name}::run();\n\
         }}\n"
    )
}

#[allow(clippy::too_many_lines)]
fn render_shell_lib_rs(package_name: &str) -> String {
    format!(
        r#"//! Tauri desktop shell for {package_name}.
//!
//! Lifecycle:
//!   1. Bind loopback:0 to find a free ephemeral port (no hardcoded port collision).
//!      Note: there is a brief window between dropping the listener and the sidecar
//!      binding the port; in practice this race is extremely rare on loopback.
//!   2. Spawn the autumn server sidecar with `AUTUMN_SERVER__PORT` set to that port.
//!      `AUTUMN_MANAGED_PG_DATA_DIR` is set to `<app-data-dir>/db` so the managed
//!      Postgres cluster (autumn feature #1119) persists across restarts.
//!      `AUTUMN_MANAGED_PG_ATTACH_URL` is cleared so an inherited attach URL cannot
//!      redirect the sidecar to a foreign cluster instead of the bundled one.
//!   3. Poll GET /health in a background thread until the server is ready (up to 30 s),
//!      then open the webview window pointing at http://127.0.0.1:<port>.
//!      On timeout, the app exits with a non-zero code rather than showing a blank window.
//!   4. On main window close, send SIGTERM for graceful shutdown (so on_shutdown hooks
//!      run, including ManagedPostgresPoolProvider::stop()), then force-kill after 3 s.

use std::net::TcpListener;
use tauri::{{Manager, App}};
use tauri_plugin_shell::{{ShellExt, process::CommandChild}};

struct SidecarHandle(std::sync::Mutex<Option<CommandChild>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {{
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(SidecarHandle(std::sync::Mutex::new(None)))
        .setup(|app| setup(app))
        .on_window_event(|window, event| {{
            if let tauri::WindowEvent::Destroyed = event {{
                // Only shut down the sidecar when the main window closes, not on
                // secondary windows (dialogs, settings panels, etc.).
                if window.label() == "main" {{
                    let handle = window.app_handle();
                    if let Some(child) = handle
                        .state::<SidecarHandle>()
                        .0
                        .lock()
                        .unwrap()
                        .take()
                    {{
                        // Send a graceful shutdown signal so autumn's on_shutdown
                        // hooks run (including ManagedPostgresPoolProvider::stop()).
                        // Fall back to force-kill after 3 s on all platforms.
                        let graceful_pid = child.pid();
                        std::thread::spawn(move || {{
                            // Unix: SIGTERM triggers autumn's tokio signal handler.
                            #[cfg(unix)]
                            {{
                                let _ = std::process::Command::new("kill")
                                    .args(["-TERM", &graceful_pid.to_string()])
                                    .status();
                            }}
                            // Windows: taskkill without /f sends CTRL_CLOSE_EVENT /
                            // WM_CLOSE for a graceful-termination attempt.
                            #[cfg(windows)]
                            {{
                                let _ = std::process::Command::new("taskkill")
                                    .args(["/pid", &graceful_pid.to_string()])
                                    .status();
                            }}
                            std::thread::sleep(std::time::Duration::from_secs(3));
                            let _ = child.kill();
                        }});
                    }}
                }}
            }}
        }})
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {{
    // 1. Find a free loopback port: bind :0, read the assigned port, then drop
    //    the listener so the autumn server can bind that same address.
    //    Note: there is a brief window between dropping the listener and the sidecar
    //    binding; in practice this race is extremely rare on loopback.
    let port = {{
        let l = TcpListener::bind("127.0.0.1:0")?;
        l.local_addr()?.port()
    }};

    // 2. Persistent data dir for the managed Postgres cluster (#1119).
    //    Use a `db/` subdirectory so Postgres cluster files don't clutter
    //    the app-data root.  Create it proactively; the sidecar won't if absent.
    let app_data_dir = app.path().app_data_dir()?.join("db");
    std::fs::create_dir_all(&app_data_dir)?;

    // autumn.toml is bundled as a Tauri resource (see tauri.conf.json bundle.resources).
    // The sidecar's working directory is set to resource_dir so AutumnConfig finds it.
    //
    // Why CWD and not AUTUMN_MANIFEST_DIR env var:
    //   OsEnv::var("AUTUMN_MANIFEST_DIR") returns the compile-time CARGO_MANIFEST_DIR
    //   set by #[autumn_web::main], overriding the process environment.  That path
    //   doesn't exist on the installed machine, so find_config_file_named() falls back
    //   to PathBuf::from("autumn.toml") — i.e. the current working directory.
    //   Setting CWD to resource_dir makes that CWD fallback find the bundled config.
    let resource_dir = app.path().resource_dir()?;

    // 3. Spawn the autumn server sidecar (built with autumn-web/embed-assets + managed-pg-bundled).
    //    The sidecar() argument is the binary basename matching externalBin in tauri.conf.json.
    let (_rx, child) = app
        .shell()
        .sidecar("{package_name}")?
        // Working directory = resource dir so autumn.toml is found via CWD fallback.
        .current_dir(&resource_dir)
        .env("AUTUMN_SERVER__HOST", "127.0.0.1")
        .env("AUTUMN_SERVER__PORT", port.to_string())
        .env(
            "AUTUMN_MANAGED_PG_DATA_DIR",
            app_data_dir.to_string_lossy().as_ref(),
        )
        // Clear any inherited attach URL so the sidecar owns its bundled Postgres
        // cluster rather than connecting to a stale or foreign database.
        // ManagedPostgresPoolProvider checks AUTUMN_MANAGED_PG_ATTACH_URL before
        // AUTUMN_MANAGED_PG_DATA_DIR and returns it without starting a local cluster;
        // an empty value is ignored by the provider.
        .env("AUTUMN_MANAGED_PG_ATTACH_URL", "")
        // Belt-and-suspenders for apps not using #[autumn_web::main] where
        // AUTUMN_MANIFEST_DIR env var IS consulted before the CWD fallback.
        .env(
            "AUTUMN_MANIFEST_DIR",
            resource_dir.to_string_lossy().as_ref(),
        )
        // Clear any inherited Unix-socket config so the sidecar always binds
        // TCP on the loopback address the probe polls.  Without this, an
        // inherited AUTUMN_SERVER__UNIX_SOCKET or AUTUMN_SERVE_FORCE_UNIX_SOCKET
        // would make the sidecar bind a socket path while the TCP health probe
        // times out and exits.
        .env("AUTUMN_SERVER__UNIX_SOCKET", "")
        .env("AUTUMN_SERVE_FORCE_UNIX_SOCKET", "")
        .spawn()?;
    *app.state::<SidecarHandle>().0.lock().unwrap() = Some(child);

    // 4. Poll for server readiness in a background thread so setup() returns immediately
    //    and the Tauri event loop starts.  Blocking here freezes the UI and can trigger
    //    OS ANR watchdogs on macOS and Windows.
    //    We probe the root path and accept ANY valid HTTP response (any status code) as
    //    the readiness signal.  This avoids depending on a specific route path, which
    //    would conflict if the app has a custom GET /health or configures [health].path
    //    differently (Axum panics on duplicate route registration).
    let handle = app.handle().clone();
    std::thread::spawn(move || {{
        // Build SocketAddr directly to avoid repeated string formatting and parse() panics.
        let addr = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            port,
        );
        let poll_timeout = std::time::Duration::from_millis(200);
        let mut ready = false;
        // 150 × 200 ms = 30 s total — enough headroom for cold Postgres initialisation.
        for _ in 0..150 {{
            if let Ok(mut stream) =
                std::net::TcpStream::connect_timeout(&addr, poll_timeout)
            {{
                // Bound the read so a silent connection doesn't stall the loop.
                let _ = stream.set_read_timeout(Some(poll_timeout));
                use std::io::{{Read, Write}};
                let req =
                    "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
                if stream.write_all(req.as_bytes()).is_ok() {{
                    let mut buf = [0u8; 8];
                    // Any valid HTTP response (200, 301, 401, 404, …) means the server
                    // is up and routing — accept the `HTTP/` prefix regardless of status.
                    if stream.read(&mut buf).is_ok() && buf.starts_with(b"HTTP/") {{
                        ready = true;
                        break;
                    }}
                }}
            }}
            std::thread::sleep(poll_timeout);
        }}
        if !ready {{
            eprintln!(
                "[{package_name}] Server did not become ready within 30 s — exiting."
            );
            // No window has been created yet, so WindowEvent::Destroyed cannot
            // fire.  Kill the sidecar explicitly before exiting so no orphaned
            // server process is left behind.
            if let Some(mut child) = handle
                .state::<SidecarHandle>()
                .0
                .lock()
                .unwrap()
                .take()
            {{
                let _ = child.kill();
            }}
            handle.exit(1);
            return;
        }}
        if let Err(e) = tauri::WebviewWindowBuilder::new(
            &handle,
            "main",
            tauri::WebviewUrl::External(
                format!("http://127.0.0.1:{{port}}").parse().unwrap(),
            ),
        )
        .title("{package_name}")
        .inner_size(1200.0, 800.0)
        .build()
        {{
            eprintln!("[{package_name}] Failed to open window: {{e}}");
            // The window was never created so Destroyed cannot clean up; kill
            // the sidecar here too.
            if let Some(mut child) = handle
                .state::<SidecarHandle>()
                .0
                .lock()
                .unwrap()
                .take()
            {{
                let _ = child.kill();
            }}
            handle.exit(1);
        }}
    }});

    Ok(())
}}
"#
    )
}

fn render_placeholder_icon_svg() -> String {
    concat!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 512 512\">\n",
        "  <!-- Placeholder app icon. Replace with your own, then run:\n",
        "       cargo tauri icon static/icons/icon.svg -->\n",
        "  <rect width=\"512\" height=\"512\" rx=\"64\" fill=\"#4F7942\"/>\n",
        "  <text x=\"256\" y=\"370\" font-size=\"280\" text-anchor=\"middle\"",
        " font-family=\"system-ui\">&#x1F342;</text>\n",
        "</svg>\n",
    )
    .to_owned()
}

fn render_stage_sidecar_sh(package_name: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# Build the autumn server sidecar with embedded assets and managed Postgres,
# then place it in src-tauri/binaries/ for Tauri to bundle.
#
# Wired into tauri.conf.json > build.beforeBuildCommand.
# Run manually: bash src-tauri/stage-sidecar.sh
#
# Requires autumn features:
#   autumn-web/embed-assets        (#1004 — single-binary asset embed)
#   autumn-web/managed-pg-bundled  (#1119 — bundled Postgres, no external install)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${{BASH_SOURCE[0]}}")" && pwd)"
APP_DIR="$(dirname "$SCRIPT_DIR")"
cd "$APP_DIR"
# TAURI_ENV_TARGET_TRIPLE is set by `cargo tauri build` for cross-compilation;
# fall back to the host triple when running the script manually.
TARGET_TRIPLE="${{TAURI_ENV_TARGET_TRIPLE:-$(rustc -Vv | awk '/^host/{{print $2}}')}}";
# Resolve the real Cargo output directory.  Workspace members share the workspace
# root's target/ and CARGO_TARGET_DIR / .cargo/config.toml can redirect it.
TARGET_DIR="${{CARGO_TARGET_DIR:-$(cargo metadata --no-deps --format-version 1 --quiet \
    | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')}}"
mkdir -p src-tauri/binaries
# universal-apple-darwin is a Tauri meta-target, not a rustc triple.  Build both
# Darwin slices separately and combine with lipo(1) into a fat binary.
if [ "${{TARGET_TRIPLE}}" = "universal-apple-darwin" ]; then
    for ARCH in x86_64-apple-darwin aarch64-apple-darwin; do
        cargo build --release --target "$ARCH" \
          --features autumn-web/embed-assets,autumn-web/managed-pg-bundled
    done
    lipo -create -output "src-tauri/binaries/{package_name}-universal-apple-darwin" \
      "${{TARGET_DIR}}/x86_64-apple-darwin/release/{package_name}" \
      "${{TARGET_DIR}}/aarch64-apple-darwin/release/{package_name}"
    echo "Staged (universal): src-tauri/binaries/{package_name}-universal-apple-darwin"
else
    # Build with both autumn-web features so the sidecar binary embeds static assets
    # and bundles Postgres.  Both are specified via the dependency path so this script
    # works with any autumn project regardless of whether the app's Cargo.toml defines
    # a top-level `embed-assets` feature alias.
    cargo build --release --target "${{TARGET_TRIPLE}}" \
      --features autumn-web/embed-assets,autumn-web/managed-pg-bundled
    cp "${{TARGET_DIR}}/${{TARGET_TRIPLE}}/release/{package_name}" \
       "src-tauri/binaries/{package_name}-${{TARGET_TRIPLE}}"
    echo "Staged: src-tauri/binaries/{package_name}-${{TARGET_TRIPLE}}"
fi
# Stage profile config files so tauri.conf.json's static resource entries are
# always satisfiable at bundle time, regardless of when the files were created
# relative to when `autumn generate tauri` was run.  An empty TOML file is
# valid and results in no overrides; AutumnConfig treats it as a no-op.
mkdir -p src-tauri/configs
for f in autumn-prod.toml autumn-production.toml autumn-staging.toml autumn-development.toml autumn-test.toml; do
    if [ -f "$f" ]; then
        cp "$f" "src-tauri/configs/$f"
    else
        : > "src-tauri/configs/$f"
    fi
done
"#
    )
}

fn render_stage_sidecar_ps1(package_name: &str) -> String {
    format!(
        r#"# Build the autumn server sidecar with embedded assets and managed Postgres,
# then place it in src-tauri\binaries\ for Tauri to bundle.
#
# Run manually: powershell -File src-tauri\stage-sidecar.ps1
# Or set tauri.conf.json > build.beforeBuildCommand to:
#   "powershell -ExecutionPolicy Bypass -File stage-sidecar.ps1"
$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$AppDir = Split-Path -Parent $ScriptDir
Set-Location $AppDir
# TAURI_ENV_TARGET_TRIPLE is set by `cargo tauri build` for cross-compilation;
# fall back to the host triple when running the script manually.
$TargetTriple = $Env:TAURI_ENV_TARGET_TRIPLE
if (-not $TargetTriple) {{
    $TargetTriple = (rustc -Vv | Select-String "^host").Line.Split()[1]
}}
# Resolve the real Cargo output directory.  Workspace members share the workspace
# root's target\ and CARGO_TARGET_DIR / .cargo/config.toml can redirect it.
$TargetDir = $Env:CARGO_TARGET_DIR
if (-not $TargetDir) {{
    $TargetDir = (cargo metadata --no-deps --format-version 1 --quiet | ConvertFrom-Json).target_directory
}}
# Build with both autumn-web features so the sidecar binary embeds static assets
# and bundles Postgres.  Both are specified via the dependency path so this script
# works with any autumn project regardless of whether the app's Cargo.toml defines
# a top-level `embed-assets` feature alias.
cargo build --release --target "$TargetTriple" `
  --features autumn-web/embed-assets,autumn-web/managed-pg-bundled
New-Item -ItemType Directory -Force -Path src-tauri\binaries | Out-Null
Copy-Item "$TargetDir\$TargetTriple\release\{package_name}.exe" `
          "src-tauri\binaries\{package_name}-$TargetTriple.exe"
Write-Host "Staged: src-tauri/binaries/{package_name}-$TargetTriple.exe"
# Stage profile config files so tauri.conf.json's static resource entries are
# always satisfiable at bundle time, regardless of when the files were created
# relative to when `autumn generate tauri` was run.  An empty TOML file is
# valid and results in no overrides; AutumnConfig treats it as a no-op.
New-Item -ItemType Directory -Force -Path src-tauri\configs | Out-Null
foreach ($f in @("autumn-prod.toml", "autumn-production.toml", "autumn-staging.toml", "autumn-development.toml", "autumn-test.toml")) {{
    if (Test-Path $f) {{
        Copy-Item $f "src-tauri\configs\$f"
    }} else {{
        New-Item -ItemType File -Force -Path "src-tauri\configs\$f" | Out-Null
    }}
}}
"#
    )
}

fn render_gitignore() -> String {
    "/target\n/binaries\n/configs\n/gen\n".to_owned()
}

/// Human-readable prerequisites message printed after a successful scaffold.
pub fn render_prerequisites() -> String {
    "\
Required prerequisites for `cargo tauri build`:\n\
\n\
  1. Tauri CLI:\n\
       cargo install tauri-cli --version '^2'\n\
\n\
  2. Platform toolchain:\n\
       Linux:   sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget \\\n\
                  file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev\n\
       macOS:   xcode-select --install\n\
       Windows: Install WebView2 + Visual Studio C++ build tools\n\
\n\
  3. Stage the autumn server sidecar (also wired into beforeBuildCommand):\n\
       bash src-tauri/stage-sidecar.sh\n\
\n\
  4. Build the desktop app:\n\
       cd src-tauri && cargo tauri build\n\
\n\
  The sidecar is built with autumn-web/embed-assets (#1004) and\n\
  autumn-web/managed-pg-bundled (#1119).  For a DB-backed app the bundled\n\
  Postgres only activates if ManagedPostgresPoolProvider is wired in your\n\
  app's pool configuration (see docs/guide/managed-pg.md).\n\
\n\
  Replace the placeholder icons before shipping:\n\
       cargo tauri icon static/icons/icon.svg   (from the app root)\n"
        .to_owned()
}

// ── Placeholder icon bytes ────────────────────────────────────────────────────
// Minimal valid 1×1 RGBA PNG (autumn green #4F7942, opaque).
// Replace with proper icons using: cargo tauri icon static/icons/icon.svg
const PLACEHOLDER_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
    0x00, 0x00, 0x00, 0x0d, // IHDR length = 13
    0x49, 0x48, 0x44, 0x52, // "IHDR"
    0x00, 0x00, 0x00, 0x01, // width = 1
    0x00, 0x00, 0x00, 0x01, // height = 1
    0x08, 0x06, 0x00, 0x00, 0x00, // depth=8, colortype=6(RGBA), compress=filter=interlace=0
    0x1f, 0x15, 0xc4, 0x89, // IHDR CRC
    0x00, 0x00, 0x00, 0x0d, // IDAT length = 13
    0x49, 0x44, 0x41, 0x54, // "IDAT"
    0x78, 0x9c, 0x63, 0xf0, 0xaf, 0x74, 0xfa, 0x0f, 0x00, 0x04, 0x2f, 0x02, 0x0a, // deflate
    0x5e, 0x60, 0x4a, 0x2d, // IDAT CRC
    0x00, 0x00, 0x00, 0x00, // IEND length = 0
    0x49, 0x45, 0x4e, 0x44, // "IEND"
    0xae, 0x42, 0x60, 0x82, // IEND CRC
];

// Minimal ICO wrapping the placeholder PNG.
const PLACEHOLDER_ICO: &[u8] = &[
    0x00, 0x00, 0x01, 0x00, // ICO header: reserved=0, type=1(ICO)
    0x01, 0x00, // image count = 1
    0x00, 0x00, 0x00, 0x00, // width=0(→256), height=0(→256), palette=0, reserved=0
    0x01, 0x00, 0x20, 0x00, // planes=1, bit_count=32
    0x46, 0x00, 0x00, 0x00, // image data size = 70 bytes
    0x16, 0x00, 0x00, 0x00, // image data offset = 22 (6+16)
    // PNG data (same as PLACEHOLDER_PNG, 70 bytes)
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf0, 0xaf, 0x74, 0xfa,
    0x0f, 0x00, 0x04, 0x2f, 0x02, 0x0a, 0x5e, 0x60, 0x4a, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

// Minimal ICNS container wrapping the placeholder PNG as icp6 (PNG icon).
const PLACEHOLDER_ICNS: &[u8] = &[
    0x69, 0x63, 0x6e, 0x73, // "icns" magic
    0x00, 0x00, 0x00, 0x56, // total file size = 86
    0x69, 0x63, 0x70, 0x36, // icon type "icp6" (PNG icon)
    0x00, 0x00, 0x00, 0x4e, // entry size = 78 (8 header + 70 PNG)
    // PNG data (same as PLACEHOLDER_PNG, 70 bytes)
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf0, 0xaf, 0x74, 0xfa,
    0x0f, 0x00, 0x04, 0x2f, 0x02, 0x0a, 0x5e, 0x60, 0x4a, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::generate::Flags;

    // ── Fixtures ──────────────────────────────────────────────────────────────

    fn project(name: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            format!(
                "[package]\nname=\"{name}\"\nversion=\"0.1.0\"\nedition=\"2024\"\n\
                 \n[dependencies]\nautumn-web = \"0.5.0\"\n"
            ),
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        tmp
    }

    // ── plan_tauri: error cases ───────────────────────────────────────────────

    #[test]
    fn plan_tauri_requires_project_root() {
        let tmp = TempDir::new().unwrap();
        let err = plan_tauri(tmp.path()).unwrap_err();
        assert!(matches!(err, GenerateError::NotInProject));
    }

    #[test]
    fn plan_tauri_errors_when_package_name_missing() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
        let err = plan_tauri(tmp.path()).unwrap_err();
        assert!(
            matches!(err, GenerateError::Config(_)),
            "expected Config error, got: {err:?}"
        );
    }

    // ── plan_tauri: file list ─────────────────────────────────────────────────

    fn has_action(tmp: &TempDir, suffix: &str) -> bool {
        let plan = plan_tauri(tmp.path()).unwrap();
        plan.actions.iter().any(|a| {
            a.path()
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with(suffix)
        })
    }

    #[test]
    fn plan_creates_tauri_conf_json() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/tauri.conf.json"));
    }

    #[test]
    fn plan_creates_shell_cargo_toml() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/Cargo.toml"));
    }

    #[test]
    fn plan_creates_build_rs() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/build.rs"));
    }

    #[test]
    fn plan_creates_src_main_rs() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/src/main.rs"));
    }

    #[test]
    fn plan_creates_src_lib_rs() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/src/lib.rs"));
    }

    #[test]
    fn plan_creates_stage_sidecar_sh() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/stage-sidecar.sh"));
    }

    #[test]
    fn plan_creates_stage_sidecar_ps1() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/stage-sidecar.ps1"));
    }

    #[test]
    fn stage_sidecar_sh_uses_cargo_metadata_for_target_dir() {
        let sh = render_stage_sidecar_sh("my-app");
        // cargo metadata resolves the real output dir for workspace members and
        // respects CARGO_TARGET_DIR overrides.
        assert!(
            sh.contains("cargo metadata"),
            "stage-sidecar.sh must use `cargo metadata` to locate the Cargo target directory"
        );
        assert!(
            sh.contains("TARGET_DIR"),
            "stage-sidecar.sh must use a TARGET_DIR variable derived from cargo metadata"
        );
        // The hardcoded relative path is no longer used — workspace builds would
        // look in the wrong place if we still had `target/$TARGET_TRIPLE/...`.
        assert!(
            !sh.contains(r#""target/$"#) && !sh.contains("\"target/${"),
            "stage-sidecar.sh must not use a hardcoded `target/` prefix for the copy"
        );
    }

    #[test]
    fn stage_sidecar_sh_handles_universal_apple_darwin() {
        let sh = render_stage_sidecar_sh("my-app");
        // universal-apple-darwin is a Tauri meta-target; cargo build --target
        // universal-apple-darwin fails because rustc doesn't know it.
        assert!(
            sh.contains("universal-apple-darwin"),
            "stage-sidecar.sh must detect universal-apple-darwin and handle it separately"
        );
        assert!(
            sh.contains("lipo"),
            "stage-sidecar.sh must combine Darwin slices with lipo for universal builds"
        );
        assert!(
            sh.contains("x86_64-apple-darwin") && sh.contains("aarch64-apple-darwin"),
            "stage-sidecar.sh must build both x86_64 and aarch64 Darwin slices"
        );
    }

    #[test]
    fn stage_sidecar_ps1_uses_cargo_metadata_for_target_dir() {
        let ps1 = render_stage_sidecar_ps1("my-app");
        assert!(
            ps1.contains("cargo metadata"),
            "stage-sidecar.ps1 must use `cargo metadata` to locate the Cargo target directory"
        );
        assert!(
            ps1.contains("TargetDir"),
            "stage-sidecar.ps1 must use a $TargetDir variable derived from cargo metadata"
        );
        assert!(
            ps1.contains("ConvertFrom-Json"),
            "stage-sidecar.ps1 must use ConvertFrom-Json to properly decode the JSON path"
        );
    }

    #[test]
    fn stage_sidecar_sh_stages_profile_configs() {
        let sh = render_stage_sidecar_sh("my-app");
        // Profile configs are staged at build time (beforeBuildCommand) so that the
        // static resource entries in tauri.conf.json are always satisfiable — even when
        // autumn-prod.toml is created after `autumn generate tauri` was run.
        assert!(
            sh.contains("src-tauri/configs"),
            "stage-sidecar.sh must create src-tauri/configs/ and populate it with \
             profile config files (or empty stubs) so tauri.conf.json resource entries resolve"
        );
        assert!(
            sh.contains("autumn-prod.toml"),
            "stage-sidecar.sh must stage autumn-prod.toml into configs/"
        );
    }

    #[test]
    fn stage_sidecar_ps1_stages_profile_configs() {
        let ps1 = render_stage_sidecar_ps1("my-app");
        assert!(
            ps1.contains("src-tauri\\configs") || ps1.contains("src-tauri/configs"),
            "stage-sidecar.ps1 must create src-tauri\\configs\\ and populate it with \
             profile config files (or empty stubs)"
        );
        assert!(
            ps1.contains("autumn-prod.toml"),
            "stage-sidecar.ps1 must stage autumn-prod.toml into configs\\"
        );
    }

    #[test]
    fn plan_creates_gitignore() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/.gitignore"));
    }

    #[test]
    fn plan_creates_icon_svg() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/icons/icon.svg"));
    }

    #[test]
    fn plan_creates_png_icons() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/icons/32x32.png"));
        assert!(has_action(&tmp, "src-tauri/icons/128x128.png"));
        assert!(has_action(&tmp, "src-tauri/icons/128x128@2x.png"));
        assert!(has_action(&tmp, "src-tauri/icons/icon.png"));
    }

    #[test]
    fn plan_creates_ico_and_icns() {
        let tmp = project("my-app");
        assert!(has_action(&tmp, "src-tauri/icons/icon.ico"));
        assert!(has_action(&tmp, "src-tauri/icons/icon.icns"));
    }

    // ── render_tauri_conf ─────────────────────────────────────────────────────

    #[test]
    fn tauri_conf_is_valid_json() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value =
            serde_json::from_str(&conf).expect("tauri.conf.json must be valid JSON");
        assert!(parsed.is_object());
    }

    #[test]
    fn tauri_conf_has_identifier() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        assert!(
            parsed["identifier"].is_string(),
            "tauri.conf.json must have an identifier"
        );
        assert!(
            parsed["identifier"].as_str().unwrap().contains("my-app"),
            "identifier must include the package name"
        );
    }

    #[test]
    fn tauri_conf_has_product_name() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        assert!(
            parsed["productName"].is_string(),
            "tauri.conf.json must have productName"
        );
    }

    #[test]
    fn tauri_conf_has_external_bin() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        let bins = parsed["bundle"]["externalBin"]
            .as_array()
            .expect("bundle.externalBin must be an array");
        assert!(!bins.is_empty(), "must list at least one external binary");
        assert!(
            bins.iter()
                .any(|b| b.as_str().unwrap_or("").contains("my-app")),
            "externalBin must reference the app name"
        );
    }

    #[test]
    fn tauri_conf_has_icon_array() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        let icons = parsed["bundle"]["icon"]
            .as_array()
            .expect("bundle.icon must be an array");
        assert!(
            icons.len() >= 4,
            "must list at least 4 icon files, got {}",
            icons.len()
        );
    }

    #[test]
    fn tauri_conf_has_before_build_command() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        let cmd = parsed["build"]["beforeBuildCommand"]
            .as_str()
            .expect("build.beforeBuildCommand must be a string");
        assert!(
            cmd.contains("stage-sidecar"),
            "beforeBuildCommand must reference the staging script"
        );
    }

    // ── render_shell_cargo_toml ───────────────────────────────────────────────

    #[test]
    fn shell_cargo_toml_has_own_workspace_table() {
        let cargo = render_shell_cargo_toml("my-app");
        assert!(
            cargo.contains("[workspace]"),
            "shell Cargo.toml must have its own [workspace] table to be independent"
        );
    }

    #[test]
    fn shell_cargo_toml_has_tauri_dep() {
        let cargo = render_shell_cargo_toml("my-app");
        assert!(
            cargo.contains("tauri"),
            "shell Cargo.toml must depend on tauri"
        );
    }

    #[test]
    fn shell_cargo_toml_has_tauri_plugin_shell_dep() {
        let cargo = render_shell_cargo_toml("my-app");
        assert!(
            cargo.contains("tauri-plugin-shell"),
            "shell Cargo.toml must depend on tauri-plugin-shell"
        );
    }

    #[test]
    fn shell_cargo_toml_has_tauri_build_dep() {
        let cargo = render_shell_cargo_toml("my-app");
        assert!(
            cargo.contains("tauri-build"),
            "shell Cargo.toml must depend on tauri-build in build-dependencies"
        );
    }

    #[test]
    fn shell_cargo_toml_package_name_includes_app_name() {
        let cargo = render_shell_cargo_toml("my-app");
        assert!(
            cargo.contains("my-app"),
            "shell crate name must reference the app name"
        );
    }

    // ── render_shell_lib_rs (sidecar lifecycle) ───────────────────────────────

    #[test]
    fn lib_rs_binds_loopback_ephemeral_port() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("127.0.0.1:0"),
            "lib.rs must bind loopback:0 to find a free ephemeral port"
        );
    }

    #[test]
    fn lib_rs_sets_autumn_server_port_env() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("AUTUMN_SERVER__PORT"),
            "lib.rs must pass AUTUMN_SERVER__PORT to the sidecar"
        );
    }

    #[test]
    fn lib_rs_sets_autumn_server_host_env() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("AUTUMN_SERVER__HOST"),
            "lib.rs must pass AUTUMN_SERVER__HOST to the sidecar"
        );
    }

    #[test]
    fn lib_rs_sets_managed_pg_data_dir() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("AUTUMN_MANAGED_PG_DATA_DIR"),
            "lib.rs must pass AUTUMN_MANAGED_PG_DATA_DIR for managed Postgres (#1119)"
        );
    }

    #[test]
    fn lib_rs_spawns_sidecar() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains(".sidecar("),
            "lib.rs must spawn the sidecar via tauri-plugin-shell"
        );
        assert!(
            lib.contains("my-app"),
            "lib.rs must reference the app name as the sidecar binary"
        );
    }

    #[test]
    fn lib_rs_polls_for_http_response() {
        let lib = render_shell_lib_rs("my-app");
        // Probe accepts any valid HTTP response rather than a specific path/status,
        // so it works regardless of the app's health route configuration.
        assert!(
            lib.contains("HTTP/"),
            "lib.rs readiness probe must accept any HTTP response prefix"
        );
        assert!(
            lib.contains("GET /"),
            "lib.rs must send a GET request to probe server readiness"
        );
    }

    #[test]
    fn lib_rs_kills_sidecar_on_window_destroyed() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("Destroyed"),
            "lib.rs must handle WindowEvent::Destroyed"
        );
        assert!(
            lib.contains(".kill()"),
            "lib.rs must kill the sidecar child on window close"
        );
        assert!(
            lib.contains("window.label()") && lib.contains("\"main\""),
            "lib.rs must guard kill behind window.label() == \"main\" so secondary windows \
             don't prematurely terminate the sidecar"
        );
    }

    #[test]
    fn lib_rs_pg_data_dir_uses_db_subdirectory() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains(".join(\"db\")"),
            "lib.rs must isolate Postgres files in <app-data-dir>/db, not the root"
        );
    }

    #[test]
    fn lib_rs_does_not_override_health_path() {
        let lib = render_shell_lib_rs("my-app");
        // Setting AUTUMN_HEALTH__PATH=/health can cause Axum to panic when the app
        // already has a custom GET /health route (duplicate route registration).
        // The probe instead accepts any HTTP response so no specific path is needed.
        assert!(
            !lib.contains("AUTUMN_HEALTH__PATH"),
            "lib.rs must NOT set AUTUMN_HEALTH__PATH — overriding it can conflict with \
             app-defined routes and cause Axum to panic on duplicate registration"
        );
    }

    #[test]
    fn lib_rs_clears_unix_socket_env() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("AUTUMN_SERVER__UNIX_SOCKET"),
            "lib.rs must clear AUTUMN_SERVER__UNIX_SOCKET so an inherited env var \
             cannot redirect the sidecar to a Unix socket the TCP probe cannot reach"
        );
        // AUTUMN_SERVE_FORCE_UNIX_SOCKET is a separate out-of-band override in
        // app.rs that bypasses the config-system socket setting; must also be cleared.
        assert!(
            lib.contains("AUTUMN_SERVE_FORCE_UNIX_SOCKET"),
            "lib.rs must also clear AUTUMN_SERVE_FORCE_UNIX_SOCKET — it overrides \
             AUTUMN_SERVER__UNIX_SOCKET and would still redirect to a Unix socket"
        );
    }

    #[test]
    fn lib_rs_clears_managed_pg_attach_url() {
        let lib = render_shell_lib_rs("my-app");
        // If AUTUMN_MANAGED_PG_ATTACH_URL is inherited (e.g. from a terminal where
        // `autumn serve` is running), ManagedPostgresPoolProvider connects to that
        // existing cluster instead of starting the bundled one.  Clearing it ensures
        // the desktop app always owns its local database.
        assert!(
            lib.contains("AUTUMN_MANAGED_PG_ATTACH_URL"),
            "lib.rs must clear AUTUMN_MANAGED_PG_ATTACH_URL so an inherited attach \
             URL cannot redirect the sidecar to a foreign or stale database cluster"
        );
    }

    #[test]
    fn lib_rs_sends_graceful_shutdown_signal_before_kill() {
        let lib = render_shell_lib_rs("my-app");
        // Graceful shutdown lets autumn's on_shutdown hooks run (including pg.stop()).
        // Force-kill is the fallback after a timeout.  Both Unix (SIGTERM) and Windows
        // (taskkill without /f) paths must be present for cross-platform correctness.
        assert!(
            lib.contains("SIGTERM") || lib.contains("-TERM"),
            "lib.rs on_window_event must send SIGTERM on Unix before force-killing"
        );
        assert!(
            lib.contains("taskkill"),
            "lib.rs on_window_event must use taskkill on Windows for graceful shutdown"
        );
        assert!(
            lib.contains("pid()"),
            "lib.rs must call child.pid() to get the sidecar PID for signal/taskkill"
        );
    }

    #[test]
    fn lib_rs_sets_autumn_manifest_dir() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("AUTUMN_MANIFEST_DIR"),
            "lib.rs must set AUTUMN_MANIFEST_DIR to the Tauri resource dir so the \
             sidecar finds the bundled autumn.toml on the installed machine"
        );
    }

    #[test]
    fn lib_rs_sets_sidecar_cwd_to_resource_dir() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains(".current_dir("),
            "lib.rs must set the sidecar's working directory to resource_dir; \
             OsEnv::var(AUTUMN_MANIFEST_DIR) returns the compile-time CARGO_MANIFEST_DIR \
             (which is absent on installed machines), so AutumnConfig falls back to \
             PathBuf::from(\"autumn.toml\") — setting CWD to resource_dir makes that \
             fallback find the bundled autumn.toml"
        );
    }

    #[test]
    fn tauri_conf_bundles_autumn_toml_as_resource() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        // Tauri v2 resources must be a map { source_path: dest_path }, not an array.
        let resources = parsed["bundle"]["resources"]
            .as_object()
            .expect("bundle.resources must be a map (Tauri v2 schema requirement)");
        let has_autumn_toml = resources.iter().any(|(k, v)| {
            k.contains("autumn.toml") || v.as_str().is_some_and(|s| s.contains("autumn.toml"))
        });
        assert!(
            has_autumn_toml,
            "tauri.conf.json must bundle autumn.toml as a resource so the installed \
             sidecar can find the app's production configuration"
        );
        // No glob entries — globs cause GlobPathNotFound when no files match.
        let has_glob = resources.iter().any(|(k, _)| k.contains('*'));
        assert!(
            !has_glob,
            "tauri.conf.json must not emit resource glob entries — Tauri fails with \
             GlobPathNotFound when the glob matches no files (common for fresh projects)"
        );
        // Profile configs must always be included from configs/ so they are bundled
        // regardless of when the files were created relative to `autumn generate tauri`.
        // The staging script creates the files (or empty stubs) at build time.
        let has_prod = resources
            .keys()
            .any(|k| k.contains("configs/") && k.contains("prod"));
        assert!(
            has_prod,
            "tauri.conf.json must include profile config entries from configs/ so \
             autumn-prod.toml is bundled even when added after `autumn generate tauri`"
        );
    }

    #[test]
    fn lib_rs_kills_sidecar_on_timeout() {
        let lib = render_shell_lib_rs("my-app");
        // The timeout branch must kill the sidecar before handle.exit(1);
        // check that .kill() appears more than once (once for Destroyed, once for timeout).
        let kill_count = lib.matches(".kill()").count();
        assert!(
            kill_count >= 2,
            "lib.rs must kill the sidecar in both the timeout path and the window-build \
             failure path, not only in WindowEvent::Destroyed; found {kill_count} .kill() call(s)"
        );
    }

    #[test]
    fn tauri_conf_identifier_replaces_underscores() {
        let conf = render_tauri_conf("my_app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        let id = parsed["identifier"].as_str().unwrap();
        assert!(
            !id.contains('_'),
            "bundle identifier must not contain underscores (invalid per Apple spec), got: {id}"
        );
        assert!(
            id.contains("my-app"),
            "bundle identifier must use hyphens instead of underscores, got: {id}"
        );
    }

    #[test]
    fn tauri_conf_security_is_under_app() {
        let conf = render_tauri_conf("my-app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        assert!(
            parsed["app"]["security"].is_object(),
            "security config must be nested under 'app' (Tauri v2 schema requirement)"
        );
        assert!(
            parsed["security"].is_null(),
            "security must NOT appear at the top level (invalid in Tauri v2 schema)"
        );
    }

    // ── render_shell_main_rs ─────────────────────────────────────────────────

    #[test]
    fn shell_main_rs_has_windows_subsystem_attr() {
        let main = render_shell_main_rs("my-app");
        assert!(
            main.contains("windows_subsystem"),
            "main.rs must set windows_subsystem to suppress console on Windows"
        );
    }

    #[test]
    fn shell_main_rs_calls_run() {
        let main = render_shell_main_rs("my-app");
        assert!(
            main.contains("::run()"),
            "main.rs must call the lib's run() function"
        );
    }

    // ── render_prerequisites ─────────────────────────────────────────────────

    #[test]
    fn prerequisites_mentions_tauri_cli() {
        let prereq = render_prerequisites();
        assert!(
            prereq.contains("tauri-cli") || prereq.contains("cargo tauri"),
            "prerequisites must mention the Tauri CLI"
        );
    }

    #[test]
    fn prerequisites_mentions_linux_toolchain() {
        let prereq = render_prerequisites();
        assert!(
            prereq.contains("webkit2gtk") || prereq.contains("libwebkit"),
            "prerequisites must mention the Linux WebKit dependency"
        );
    }

    #[test]
    fn prerequisites_mentions_macos_toolchain() {
        let prereq = render_prerequisites();
        assert!(
            prereq.contains("xcode") || prereq.contains("Xcode"),
            "prerequisites must mention Xcode for macOS"
        );
    }

    #[test]
    fn prerequisites_mentions_embed_assets_feature() {
        let prereq = render_prerequisites();
        assert!(
            prereq.contains("embed-assets"),
            "prerequisites must document the embed-assets dependency (#1004)"
        );
    }

    #[test]
    fn prerequisites_mentions_managed_pg_feature() {
        let prereq = render_prerequisites();
        assert!(
            prereq.contains("managed-pg"),
            "prerequisites must document the managed-pg dependency (#1119)"
        );
    }

    // ── placeholder icons ─────────────────────────────────────────────────────

    #[test]
    fn placeholder_png_starts_with_png_signature() {
        assert_eq!(
            &PLACEHOLDER_PNG[..8],
            &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
            "placeholder PNG must start with the PNG file signature"
        );
    }

    #[test]
    fn placeholder_ico_starts_with_ico_header() {
        assert_eq!(
            &PLACEHOLDER_ICO[..4],
            &[0x00, 0x00, 0x01, 0x00],
            "placeholder ICO must start with the ICO reserved+type header"
        );
    }

    #[test]
    fn placeholder_icns_starts_with_icns_magic() {
        assert_eq!(
            &PLACEHOLDER_ICNS[..4],
            b"icns",
            "placeholder ICNS must start with the 'icns' magic bytes"
        );
    }

    // ── additive (does not touch app's src/main.rs or root Cargo.toml) ───────

    #[test]
    fn plan_does_not_modify_app_main_rs() {
        let tmp = project("my-app");
        let plan = plan_tauri(tmp.path()).unwrap();
        let app_main = tmp.path().join("src/main.rs");
        assert!(
            !plan.actions.iter().any(|a| a.path() == app_main.as_path()),
            "plan must not touch the app's src/main.rs"
        );
    }

    #[test]
    fn plan_does_not_modify_root_cargo_toml() {
        let tmp = project("my-app");
        let plan = plan_tauri(tmp.path()).unwrap();
        let root_cargo = tmp.path().join("Cargo.toml");
        assert!(
            !plan
                .actions
                .iter()
                .any(|a| a.path() == root_cargo.as_path()),
            "plan must not touch the root Cargo.toml"
        );
    }

    // ── icon reuse when PWA generator already ran ─────────────────────────────

    #[test]
    fn plan_reuses_pwa_icon_when_present() {
        let tmp = project("my-app");
        // Simulate that `autumn generate pwa` already created the icon
        fs::create_dir_all(tmp.path().join("static/icons")).unwrap();
        fs::write(
            tmp.path().join("static/icons/icon.svg"),
            "<svg><!-- pwa icon --></svg>\n",
        )
        .unwrap();

        let plan = plan_tauri(tmp.path()).unwrap();
        let svg_action = plan
            .actions
            .iter()
            .find(|a| {
                a.path()
                    .to_string_lossy()
                    .replace('\\', "/")
                    .ends_with("icons/icon.svg")
            })
            .expect("icon.svg must be in the plan");

        if let super::super::emit::Action::CreateIfAbsent { contents, .. } = svg_action {
            assert!(
                contents.contains("pwa icon"),
                "must reuse the PWA icon content"
            );
        } else {
            panic!("icon.svg must use CreateIfAbsent action");
        }
    }

    // ── plan execution (full write to tempdir) ────────────────────────────────

    #[test]
    fn execute_creates_tauri_conf_json() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let path = tmp.path().join("src-tauri/tauri.conf.json");
        assert!(path.exists(), "src-tauri/tauri.conf.json must be created");
        let content = fs::read_to_string(&path).unwrap();
        let _: serde_json::Value =
            serde_json::from_str(&content).expect("tauri.conf.json must be valid JSON");
    }

    #[test]
    fn execute_creates_shell_cargo_toml() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        assert!(tmp.path().join("src-tauri/Cargo.toml").exists());
    }

    #[test]
    fn execute_creates_lib_rs() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let lib = fs::read_to_string(tmp.path().join("src-tauri/src/lib.rs")).unwrap();
        assert!(
            lib.contains("127.0.0.1:0"),
            "lib.rs must bind ephemeral port"
        );
        assert!(lib.contains(".kill()"), "lib.rs must kill sidecar on close");
    }

    #[test]
    fn execute_creates_png_icon_files() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        for name in &["32x32.png", "128x128.png", "128x128@2x.png", "icon.png"] {
            let path = tmp.path().join("src-tauri/icons").join(name);
            assert!(path.exists(), "{name} must be created");
            let bytes = fs::read(&path).unwrap();
            assert_eq!(
                &bytes[..8],
                &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
                "{name} must be a valid PNG"
            );
        }
    }

    #[test]
    fn execute_creates_gitignore() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let gi = fs::read_to_string(tmp.path().join("src-tauri/.gitignore")).unwrap();
        assert!(gi.contains("/target"), ".gitignore must exclude /target");
        assert!(
            gi.contains("/binaries"),
            ".gitignore must exclude /binaries"
        );
        assert!(
            gi.contains("/configs"),
            ".gitignore must exclude /configs (staging area for profile config files)"
        );
    }

    #[test]
    fn execute_does_not_touch_app_main_rs() {
        let tmp = project("my-app");
        let original_main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let after_main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert_eq!(
            original_main, after_main,
            "src/main.rs must be unchanged after generate tauri"
        );
    }

    #[test]
    fn execute_does_not_touch_root_cargo_toml() {
        let tmp = project("my-app");
        let original_cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let after_cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert_eq!(
            original_cargo, after_cargo,
            "root Cargo.toml must be unchanged after generate tauri"
        );
    }

    #[test]
    fn execute_is_idempotent_with_force() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        // Second run with --force must not corrupt files
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags {
                force: true,
                dry_run: false,
            })
            .unwrap();
        let conf = fs::read_to_string(tmp.path().join("src-tauri/tauri.conf.json")).unwrap();
        let _: serde_json::Value = serde_json::from_str(&conf)
            .expect("tauri.conf.json must still be valid JSON after re-run");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags {
                dry_run: true,
                force: false,
            })
            .unwrap();
        assert!(
            !tmp.path().join("src-tauri").exists(),
            "dry-run must not create any files"
        );
    }

    #[test]
    fn collision_without_force_errors() {
        let tmp = project("my-app");
        plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let err = plan_tauri(tmp.path())
            .unwrap()
            .execute(Flags::default())
            .unwrap_err();
        assert!(
            matches!(err, GenerateError::Collisions(_)),
            "re-running without --force must return a Collisions error"
        );
    }

    // ── lib.rs background thread + timeout behaviour ──────────────────────────

    #[test]
    fn lib_rs_uses_background_thread_for_health_poll() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("thread::spawn"),
            "lib.rs must move the health poll into a background thread so setup() \
             returns immediately and the Tauri event loop starts"
        );
    }

    #[test]
    fn lib_rs_exits_app_on_sidecar_timeout() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains(".exit("),
            "lib.rs must call handle.exit() when the sidecar fails to become ready, \
             not silently open a blank window"
        );
    }

    #[test]
    fn lib_rs_uses_connect_timeout_for_health_poll() {
        let lib = render_shell_lib_rs("my-app");
        assert!(
            lib.contains("connect_timeout"),
            "lib.rs must use TcpStream::connect_timeout so each poll attempt is bounded"
        );
    }

    // ── productName title-case handles snake_case package names ──────────────

    #[test]
    fn tauri_conf_product_name_handles_underscore_separator() {
        let conf = render_tauri_conf("my_app");
        let parsed: serde_json::Value = serde_json::from_str(&conf).unwrap();
        let name = parsed["productName"].as_str().unwrap();
        assert!(
            name.contains(' '),
            "productName for 'my_app' must contain spaces (title-case), got: {name}"
        );
        assert!(
            !name.contains('_'),
            "productName for 'my_app' must not contain underscores, got: {name}"
        );
    }
}
