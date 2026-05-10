# Autumn Release Checklist

Use this checklist before tagging an `autumn-web` or `autumn-cli` release.

## Version Alignment

- [ ] `Cargo.toml` workspace `version` and `rust-version` match the README
  requirements and first-run docs.
- [ ] `autumn-web`, `autumn-cli`, and `autumn-macros` publish metadata point at
  the same repository, license, and release line.
- [ ] CHANGELOG entries call out any MSRV change.

## Automated Gates

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo test -p autumn-cli --test repo_hygiene`

## First-Run Docs Gate

- [ ] Run the `docs-smoke` procedure in
  [`docs/guide/docs-smoke.md`](guide/docs-smoke.md).
- [ ] Confirm the smoke uses the published `autumn-cli` install path and the
  published `autumn-web` dependency line, with no workspace patches.
- [ ] Treat any failure in the active first-run docs as a release blocker for
  both `autumn-web` and `autumn-cli`.
- [ ] If the smoke is temporarily run before crates.io publication, record the
  workspace-prepublish reason in release notes and rerun the published
  docs-smoke before announcing the release.
