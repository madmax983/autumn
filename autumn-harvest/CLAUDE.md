# autumn-harvest

Postgres-backed durable workflow orchestration engine for the Autumn web framework.
Companion to `autumn-web`. Implements Temporal-style durable execution + Airflow-style
DAG scheduling, Postgres-only, no external brokers.

## Workspace

- `autumn-harvest/` — core library (traits, event store, context, builder, Diesel models)
- `autumn-harvest-macros/` — proc macros (`#[workflow]`, `#[activity]`, `workflows![]`, etc.)

## Dependency

Requires `autumn-web` via path dep: `path = "../autumn"`. Publish order when
releasing: `autumn-harvest-macros` first, then `autumn-harvest`.

## Schema

Migrations live in `autumn-harvest/migrations/`. Run with `diesel migration run`.
Schema is hand-maintained in `autumn-harvest/src/schema.rs` — keep in sync with SQL.

## Macro Pattern

`#[workflow]` and `#[activity]` generate companion functions:
- `__autumn_workflow_info_{name}() -> WorkflowInfo`
- `__autumn_activity_info_{name}() -> ActivityInfo`

All generated code uses `::autumn_harvest::` paths — never upstream crate paths.

## Build & Test

```bash
cd autumn-harvest
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Phase Status

- Phase 1 (complete): types, error, event, policy, context stubs, models, macros
- Phase 2 (next): replay engine, task queue worker, LISTEN/NOTIFY, heartbeating
- Phase 3: DAG scheduler, signals/queries, saga pattern, management API
