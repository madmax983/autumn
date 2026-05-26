# Dev-Loop Latency Budget

Autumn's product promise is that the edit–refresh loop feels **boringly fast**.
This document defines the accepted latency budget for every change class that
`autumn dev` handles, explains how to measure it, and describes the gates that
prevent regressions from shipping undetected.

> **Quick comparison:** Rails and Phoenix developers expect saves to show up in
> under a second for non-code changes and within a few seconds for compiled
> code. Autumn can't hide Rust compile times, but it publishes budgets per
> change class so contributors can see exactly where the framework stands
> and contributors can tell immediately when a change makes things worse.

---

## Budget Matrix

All timings are **end-to-end wall-clock milliseconds** from the moment a file is
saved on disk to the moment a successful, observable result is confirmed in the
browser (refreshed stylesheet, updated route response, or successful HTTP reply
after a server restart). Process-restart log lines are not sufficient — the
budget covers what the user can actually see.

| Change class | p50 ms | p95 ms | max ms | Notes |
|---|---:|---:|---:|---|
| Initial dev boot to first route | 10 000 | 20 000 | 40 000 | Cold compile; varies widely by machine |
| Rust route edit — `examples/hello` (no DB) | 3 000 | **5 000** | 10 000 | Warm incremental compile |
| Rust route edit — database-backed example | 5 000 | **10 000** | 20 000 | Warm incremental compile + Diesel |
| CSS/Tailwind edit to refreshed stylesheet | 500 | **1 000** | 2 000 | Tailwind rebuild only, no Rust compile |
| Static asset edit to browser reload | 300 | **1 000** | 2 000 | File-copy + SSE push, no compile |
| Config edit (`autumn.toml`) to restarted server | 3 000 | 8 000 | 15 000 | Process restart; no Rust recompile |
| Custom `dev.watch_dirs` edit to restarted server | 3 000 | 8 000 | 15 000 | Same as config edit |

**Bolded p95 values are the enforced gates** referenced in the success metric
and checked by `autumn dev-loop-bench --fail-on-regression`.

### Regression allowance

Any release that **exceeds the absolute budgets above**, or **regresses an
accepted baseline by more than 20%** before the absolute budgets are met, must
either fail the documented gate or carry an explicit release-note exception.

---

## Validated Examples

Measurements are taken against at least two example projects to cover the two
main development paths:

| Path | Example | Prerequisites |
|---|---|---|
| No-database | `examples/hello` | Rust toolchain, `cargo` |
| Database-backed | `examples/todo-app` | Rust toolchain, `cargo`, running PostgreSQL |

### Running `examples/hello` (no database)

```bash
# From the workspace root
cd examples/hello
autumn dev          # leaves dev server running
# In a second terminal, run the benchmark:
autumn dev-loop-bench --example examples/hello --runs 5
```

### Running `examples/todo-app` (database-backed)

```bash
# Start a local Postgres instance
docker compose -f examples/todo-app/docker-compose.yml up -d
# Apply migrations
cd examples/todo-app && autumn migrate
# In a second terminal:
autumn dev-loop-bench --example examples/todo-app --runs 5
```

---

## Measurement Methodology

`autumn dev-loop-bench` measures **user-visible end-to-end latency**, not
internal watcher events or process-restart log timestamps.

For each change class the tool:

1. Confirms the dev server is responding on `http://localhost:3000`.
2. Records a **save timestamp** immediately before writing the file change.
3. Polls the relevant observable endpoint (HTTP route, stylesheet URL, etc.)
   at 50 ms intervals until it observes a successful updated response or the
   timeout (2× the `max_ms` budget) expires.
4. Records an **observe timestamp** when the updated response is confirmed.
5. `duration = observe_timestamp − save_timestamp`
6. Repeats `--runs` times (default 5) and computes p50, p95, and max.

**What counts as "observable":**

| Change class | Observable result |
|---|---|
| Rust route edit | HTTP 200 response body contains the changed text |
| CSS/Tailwind edit | `Content-Length` or `ETag` of the stylesheet URL changes |
| Static asset edit | `ETag` of the asset URL changes |
| Config/watch_dirs edit | HTTP 200 response after restart (liveness probe) |
| Initial boot | First HTTP 200 on root route |

---

## Environment Metadata

Every report includes enough metadata to interpret variance without leaking
local paths or secrets:

| Field | Description |
|---|---|
| `timestamp_utc` | ISO 8601 UTC timestamp of the measurement run |
| `runner_os` | Operating system identifier (`linux`, `macos`, `windows`) |
| `rust_version` | `rustc` version used for the compile steps |
| `autumn_version` | `autumn-web` crate version |
| `example_name` | Which example was benchmarked |

Local absolute paths, usernames, and environment variables are **never
included** in the report output.

---

## Running the Benchmark

### Print the budget table (no server required)

```bash
autumn dev-loop-bench --dry-run
```

### Measure against `examples/hello`

```bash
autumn dev-loop-bench --example examples/hello --runs 5
```

### Write a machine-readable report

```bash
AUTUMN_BENCH_TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ) \
AUTUMN_BENCH_RUST_VERSION=$(rustc --version) \
autumn dev-loop-bench \
  --example examples/hello \
  --runs 5 \
  --output dev-loop-report.json
```

### Emit JSON to stdout

```bash
autumn dev-loop-bench --json
```

### Fail CI on regression

```bash
autumn dev-loop-bench --fail-on-regression
# exits 1 if any change class exceeds its budget
```

---

## Report Format

### Human-readable summary (default)

```
Autumn dev-loop latency report — 2026-05-26T12:00:00Z
Runner: linux  Rust: 1.88.0  autumn-web: 0.4.0
Example: examples/hello

Change class                                        p50 ms   p95 ms   max ms  Status
-------------------------------------------------------------------------------------------
Initial dev boot to first route                      8 234   14 102   22 450  PASS
Rust route edit (examples/hello, no-DB)              1 823    3 991    7 234  PASS
CSS/Tailwind edit to refreshed stylesheet              342      712    1 101  PASS
Static asset edit to browser reload                    198      432      890  PASS
Config edit (autumn.toml) to restarted server        2 341    5 102    9 234  PASS
Custom watch_dirs edit to restarted server           2 234    4 891    8 992  PASS

Overall: PASS
```

### Machine-readable JSON

The `--output` flag writes a JSON file suitable for archiving as release
evidence. Example structure:

```json
{
  "timestamp_utc": "2026-05-26T12:00:00Z",
  "runner_os": "linux",
  "rust_version": "rustc 1.88.0 (stable)",
  "autumn_version": "0.4.0",
  "example_name": "examples/hello",
  "all_passed": true,
  "results": [
    {
      "change_class": "css_tailwind",
      "journey_name": "CSS/Tailwind edit to refreshed stylesheet",
      "stats": {
        "p50_ms": 342,
        "p95_ms": 712,
        "max_ms": 1101,
        "sample_count": 5
      },
      "budget": {
        "p50_ms": 500,
        "p95_ms": 1000,
        "max_ms": 2000
      },
      "passed": true,
      "p95_exceeded": false,
      "max_exceeded": false,
      "p95_overage_pct": 0.0,
      "diagnosis": "",
      "next_action": ""
    }
  ]
}
```

---

## Regression Diagnostics

When a change class exceeds its budget the report names the failing user
journey, diagnoses what exceeded the limit, and proposes a concrete next
action. Example failing row:

```
CSS/Tailwind edit to refreshed stylesheet  1 234  1 450  2 100  FAIL
  ↳ Journey 'CSS/Tailwind edit to refreshed stylesheet' regressed:
      p95 1450ms exceeds budget 1000ms (45% over).
  ↳ Next: Check for new CSS plugins or a slow Tailwind config glob.
          Run `autumn dev` manually and time the Tailwind step in the log.
```

### Diagnostic actions by change class

| Change class | Typical causes | Next action |
|---|---|---|
| CSS/Tailwind | New Tailwind plugins, slow glob patterns | Profile `tailwind --watch` independently |
| Static asset | New large assets, spurious reload triggers | Check watcher filter rules |
| Rust route edit | New proc-macro deps, increased monomorphisation | Run `cargo build --timings` |
| Initial boot | New blocking startup tasks, migration growth | Review `autumn dev` startup log |
| Config/watch_dirs | New blocking startup I/O | Audit app initialisation code |

---

## CI Gate

### Scheduled job (`.github/workflows/dev-loop-latency.yml`)

A weekly CI job runs `autumn dev-loop-bench --fail-on-regression` against
`examples/hello` and uploads the JSON report as a workflow artifact. The job
runs on an `ubuntu-latest` GitHub Actions runner.

For checks that are **too flaky or expensive for every PR** (live browser
polling, database-backed paths), the job is scheduled weekly and can be
triggered manually via `workflow_dispatch`. These checks are excluded from
per-PR required status checks.

### Per-PR opt-in

To run the latency gate against a specific PR branch before merging:

```bash
gh workflow run dev-loop-latency.yml --ref your-branch-name
```

### Release gate

The release checklist (`docs/release-checklist.md`) requires that the most
recent weekly latency run **passed** before a release is tagged, or that an
explicit release-note exception is documented explaining why a regression is
acceptable.

---

## Comparison with Other Frameworks

Autumn's dev-loop budget is designed to be competitive with Rails and Phoenix
for the non-Rust-compile change classes (CSS, static assets, config) while
being honest that Rust warm incremental compiles are slower than Ruby/Elixir
interpreted reloads.

| Change type | Rails/Phoenix | Autumn budget (p95) |
|---|---|---|
| CSS/static reload | < 500 ms | 1 000 ms |
| Config reload | < 2 s | 8 000 ms |
| Code change (server restart) | < 2 s | 5 000 ms (hello) / 10 000 ms (DB) |

The 5 s / 10 s Rust budgets reflect warm incremental compilation with
`cargo build`, not cold builds. Cold builds on underpowered CI runners are
explicitly excluded from the per-PR gate (see `max_ms` limits) and documented
in release notes when they regress.

---

## Out of Scope

- **Hot Module Replacement** that patches Rust handlers without a process
  restart — this is a separate feature tracked in its own issue.
- **Production runtime latency** — this document covers local development only.
- **Browser visual regression testing** — screenshot assertions are not part of
  this budget.
- **Cold compile times** — the budgets above assume a warm incremental build.
  Cold compile time is tracked separately in the runtime benchmarks.
