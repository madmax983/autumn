### Fixed

- **cli/upgrade:** `autumn upgrade` now follows the git root, not the Cargo
  workspace, when deciding who owns the CI workflow files (#2344). A crate
  that is its own git repository — a nested repo or submodule beneath an
  enclosing Cargo workspace — gets its `.github/workflows/ci.yml` and
  `posture-gate.yml` reconciled again, instead of being silently skipped on
  the strength of a Cargo ancestor that has nothing to do with workflow
  discovery. `clippy.toml`, `rustfmt.toml` and `rust-toolchain.toml` still
  follow Cargo ancestry (a crate-local copy would shadow the workspace's).
  A crate that merely sits inside the enclosing repository still has its
  workflows left alone. [no-plugin]
