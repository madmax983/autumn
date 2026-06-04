# Contributing to Autumn

## Generator conformance gate

Autumn's headline DX promise is that `autumn new` and `autumn generate` emit
code that **compiles, boots, and serves**. The tests that prove this live in
`autumn-cli/tests/`:

| Test | File | What it checks |
|------|------|----------------|
| `generated_project_compiles_runs_and_serves` | `e2e.rs` | `autumn new` → `cargo build` + HTTP responses |
| `generated_scaffold_cargo_checks` | `generate.rs` | `generate scaffold` → `cargo check --tests` |
| `generated_scaffold_config_cargo_checks` | `generate.rs` | config-driven scaffold → `cargo check --tests` |
| `generated_scaffold_serves_posts_index_and_json_api` | `generate.rs` | scaffold + Postgres migrations + live HTTP |

### Why `#[ignore]`?

These tests carry `#[ignore]` annotations so that `cargo test --workspace`
(which runs in seconds) does not block on multi-minute compile cycles in
everyday development. **The `#[ignore]` label means "CI-gated, not
abandoned."**

The `.github/workflows/generator-conformance.yml` workflow runs all four
tests explicitly via `-- --ignored --exact`. It fires on every PR or push
that touches:

- `autumn-cli/src/generate/**` (generator logic)
- `autumn-cli/src/templates/**` (scaffold/model/auth templates)
- `autumn-cli/src/new.rs` (project scaffolding)
- `autumn/src/lib.rs` or `autumn/src/prelude.rs` (public API surface)
- `autumn-macros/**` (proc-macro API surface)

A weekly scheduled run also catches breakage that arrives through transitive
dependency updates rather than direct file edits.

### Running them locally

```sh
# All ignored generator tests at once
cargo test -p autumn-cli -- --ignored

# Individual gates
cargo test -p autumn-cli --test e2e    generated_project_compiles_runs_and_serves    -- --ignored --exact
cargo test -p autumn-cli --test generate generated_scaffold_cargo_checks             -- --ignored --exact
cargo test -p autumn-cli --test generate generated_scaffold_config_cargo_checks      -- --ignored --exact
cargo test -p autumn-cli --test generate generated_scaffold_serves_posts_index_and_json_api -- --ignored --exact
```

The last test requires Docker (for the Postgres testcontainer) and the
`diesel` CLI on `PATH`.

### What triggers a failure?

Any change to the `autumn-web` public surface that the generated templates
depend on — a renamed macro argument, a moved prelude re-export, an
`AppBuilder` signature change — will cause the compiled output to fail
`cargo check`. The generator conformance gate catches this before it reaches
a user's first `autumn generate scaffold`.

The tests capture and print the full `cargo build` / `cargo check`
stdout+stderr on failure, so the breakage is diagnosable directly from the
CI summary.
