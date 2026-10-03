### Fixed

- **cli:** a scaffolded app's `build.rs` no longer trusts a *relative*
  `CARGO_TARGET_DIR` when locating the Tailwind CLI (issue #3108). Cargo
  resolves a relative `CARGO_TARGET_DIR` against the workspace root, but the
  build script runs with the member directory as CWD — so the raw env var
  resolved against the wrong base, and a stale member-local
  `target/autumn/tailwindcss` left behind by a standalone build of that one
  member was tried *first*, shadowing the real `autumn setup` install. Only
  an absolute `CARGO_TARGET_DIR` is offered as a candidate now; the
  always-absolute `OUT_DIR` ancestors are unchanged, so the lookup still
  finds the install in every other configuration.
