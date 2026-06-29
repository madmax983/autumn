# Desktop Apps with Tauri (`autumn generate tauri`)

`autumn generate tauri` scaffolds a complete `src-tauri/` sub-project that
wraps any existing autumn app in a native desktop shell. The generated project
uses Tauri v2's **sidecar model**: the autumn server binary runs as a supervised
child process and the webview loads the app from a loopback port chosen at
runtime. Your existing routes, Maud/htmx templates, and sessions run entirely
unmodified — the generator is purely additive and never rewrites your app code.

## Prerequisites

### External toolchain

Install the Tauri CLI:

```bash
cargo install tauri-cli --version "^2"
```

The Tauri CLI also requires platform-specific toolchain components:

| Platform | Required components |
|----------|-------------------|
| **Linux** | `webkit2gtk-4.1`, `build-essential`, `curl`, `wget`, `file`, `libssl-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` |
| **macOS** | Xcode Command Line Tools (`xcode-select --install`) |
| **Windows** | [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) (pre-installed on Windows 11) |

See the [Tauri prerequisites guide](https://v2.tauri.app/start/prerequisites/) for the full up-to-date list.

### autumn feature dependencies

The scaffolded desktop app relies on two autumn features that must be enabled
when building the sidecar binary:

- **`autumn-web/managed-pg-bundled`** (issue #1119 — managed local Postgres):
  embeds the Postgres binaries into the sidecar so the packaged desktop app
  needs no separately-installed database. The sidecar is pointed at a
  persistent per-app data directory via the `AUTUMN_MANAGED_PG_DATA_DIR`
  environment variable, which the generated Tauri shell sets automatically to
  `{app-data-dir}/db`.

- **`autumn-web/embed-assets`** (issue #1004 — single-binary asset embed):
  embeds the `static/` tree (CSS, JS, images, the fingerprint manifest) into
  the release binary so the packaged app has no loose files alongside it.

Both features are already enabled by the generated `stage-sidecar.sh` /
`stage-sidecar.ps1` scripts; you only need to wire `ManagedPostgresPoolProvider`
in your app's pool configuration if you haven't already (see the
[managed Postgres guide](managed-pg.md)).

## Scaffolding

Run the generator from your project root:

```bash
autumn generate tauri
```

The generator is idempotent. Re-running without `--force` errors on existing
files; re-running with `--force` regenerates them deterministically. It never
touches your app's `src/main.rs` or root `Cargo.toml`.

```
src-tauri/
  tauri.conf.json          — Tauri v2 config (productName, identifier, bundle, sidecar ref)
  Cargo.toml               — standalone shell crate with its own [workspace]
  build.rs                 — calls tauri_build::build()
  src/
    main.rs                — #![windows_subsystem = "windows"] + calls lib::run()
    lib.rs                 — sidecar lifecycle glue (see below)
  icons/
    icon.svg               — SVG source; replace with your own, then run `cargo tauri icon`
    32x32.png              }
    128x128.png            } placeholder icons; regenerate from icon.svg with `cargo tauri icon`
    128x128@2x.png         }
    icon.png               }
    icon.ico               — Windows icon
    icon.icns              — macOS icon
  stage-sidecar.sh         — Unix: build autumn sidecar → copy to binaries/
  stage-sidecar.ps1        — Windows: same in PowerShell
  .gitignore               — /target  /binaries  /gen
```

If you previously ran `autumn generate pwa`, the existing `static/icons/icon.svg`
is reused as the Tauri icon source automatically.

## Sidecar lifecycle

`src-tauri/src/lib.rs` implements the full desktop process lifecycle:

1. **Ephemeral port**: `TcpListener::bind("127.0.0.1:0")` picks an OS-assigned
   port; the port number is saved and the listener is dropped before the sidecar
   needs it. No hardcoded ports, no firewall rules.

2. **Spawn sidecar**: the Tauri `setup` hook uses `tauri-plugin-shell` to launch
   the autumn binary with `AUTUMN_SERVER__HOST=127.0.0.1`,
   `AUTUMN_SERVER__PORT={port}`, and `AUTUMN_MANAGED_PG_DATA_DIR={app-data-dir}/db`.

3. **Wait for ready**: the setup hook polls `GET /health` (the existing autumn
   health endpoint) over raw TCP until it gets an `HTTP/1.1 200` response or
   times out after 30 seconds.

4. **Open window**: once the server is ready, the webview navigates to
   `http://127.0.0.1:{port}`.

5. **Clean shutdown**: on `WindowEvent::Destroyed`, the stored `CommandChild`
   handle is killed. No orphaned server process survives after the window closes.

## Building a native installer

From inside `src-tauri/`:

```bash
# Stage the sidecar binary (called automatically by tauri build, but can run standalone):
sh stage-sidecar.sh          # Unix
.\stage-sidecar.ps1          # Windows PowerShell

# Build the installer for the host OS:
cargo tauri build
```

Installer outputs by platform:

| Platform | Output |
|----------|--------|
| Linux | `.deb`, `.rpm`, `.AppImage` under `src-tauri/target/release/bundle/` |
| macOS | `.dmg`, `.app` under `src-tauri/target/release/bundle/` |
| Windows | `.msi`, NSIS `.exe` under `src-tauri\target\release\bundle\` |

Cross-compilation is not supported by Tauri out of the box; build on each
target platform or use a CI matrix (GitHub Actions with `ubuntu-latest`,
`macos-latest`, `windows-latest`).

## Customising icons

Replace the placeholder icons with your own:

```bash
# Provide a 1024×1024 PNG or square SVG:
cp my-logo.svg src-tauri/icons/icon.svg
cd src-tauri
cargo tauri icon icons/icon.svg
```

`cargo tauri icon` regenerates all required sizes and formats automatically.

## Dev workflow

During development you can run the autumn server normally (`autumn dev` or
`cargo run`) and open the Tauri shell against it:

```bash
# Terminal 1 — run the autumn server
autumn dev

# Terminal 2 — run the Tauri shell in dev mode
cd src-tauri
cargo tauri dev
```

`tauri dev` hot-reloads the webview when the server's HTML changes; the
`beforeDevCommand` in `tauri.conf.json` is intentionally left empty so
`cargo tauri dev` does not try to start a second server instance.

## Relationship to PWA support

`autumn generate tauri` and `autumn generate pwa` are independent. Running both
gives you a Progressive Web App installable from the browser **and** a native
desktop installer from the same server-rendered codebase — no code duplication.
