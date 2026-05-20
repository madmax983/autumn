# Implementation Plan: Issue #826 - Per-Query Statement Timeouts and Slow-Query Telemetry

Implement database statement timeouts (`database.statement_timeout`) and slow-query detection/telemetry (`database.slow_query_threshold`) within `autumn-web`'s `#[repository]` and global connection checkout using a test-driven development (TDD) approach.

---

## User Review Required

> [!IMPORTANT]
> The database connection statement timeouts are executed on checked-out Postgres connections. When connections are returned to the pool, their session configurations must be safely reset (or set to the next client's desired timeout) to prevent pool parameter leakage. Our design always executes `SET statement_timeout` (either to the requested duration or to `0` to disable/reset) on every connection checkout, ensuring session state isolation.

---

## Open Questions

> [!NOTE]
> No major open questions remain. The design fully addresses all technical constraints, ensuring zero pool leakage, comprehensive scrubbing of query parameters, accurate timing telemetry, and integration with the `/actuator/metrics` Prometheus exporter.

---

## Proposed Changes

### Component: Core Configuration (`autumn/src/config.rs`)

We will add two new configuration parameters under `DatabaseConfig` and implement robust string-based duration parsing to support values like `"500ms"`, `"5s"`, `"1m"`, `"1h"`, and plain integers (treated as milliseconds).

#### [MODIFY] [config.rs](file:///C:/Users/markm/.gemini/antigravity/worktrees/autumn/implement-issue-826-tdd/autumn/src/config.rs)
- Implement `parse_duration_str(s: &str) -> Result<std::time::Duration, ParseDurationError>` supporting milliseconds, seconds, minutes, hours, and integer fallbacks.
- Implement `deserialize_duration` and `deserialize_option_duration` Serde helpers.
- Add fields to `DatabaseConfig`:
  - `statement_timeout: Option<Duration>`
  - `slow_query_threshold: Duration` (default 500ms)

---

### Component: Database Connections & Extractor (`autumn/src/db.rs`)

We will introduce the route-level override extractor, extend `DbState` with getters for timeouts, and apply `SET statement_timeout` inside connection checkouts. We will also implement the instrumented query runner with SQL scrubbing.

#### [MODIFY] [db.rs](file:///C:/Users/markm/.gemini/antigravity/worktrees/autumn/implement-issue-826-tdd/autumn/src/db.rs)
- Add `StatementTimeout(pub std::time::Duration)` extractor.
- Extend `DbState` with `statement_timeout() -> Option<Duration>` and `slow_query_threshold() -> Duration`.
- In `FromRequestParts` for `Db`, check for request-scoped `StatementTimeout` override and the `MatchedPath` extensions.
- When acquiring a connection from `pool.get()`, execute `SET statement_timeout = <ms>` (using the override, the default, or `0` for none/reset).
- Implement `scrub_sql(sql: &str) -> String` to replace parameters and literal strings/numbers with `?` while leaving placeholders like `$1` intact.
- Implement query execution wrappers that record duration, log slow queries, register metrics in `MetricsCollector`, and map SQLState `"57014"` (query cancellation/timeout) to `AutumnError::query_timeout`.

---

### Component: Application State (`autumn/src/state.rs`)

We will implement `DbState` getters on `AppState` by fetching the configured values from the active `AutumnConfig` extension.

#### [MODIFY] [state.rs](file:///C:/Users/markm/.gemini/antigravity/worktrees/autumn/implement-issue-826-tdd/autumn/src/state.rs)
- Implement `statement_timeout()` and `slow_query_threshold()` in `impl DbState for AppState`.

---

### Component: Macros (`autumn-macros/src/repository.rs`)

We will update the repository generator macro to inject telemetry metadata into the generated repository structs and wrap query execution inside the instrumented telemetry helper.

#### [MODIFY] [repository.rs](file:///C:/Users/markm/.gemini/antigravity/worktrees/autumn/implement-issue-826-tdd/autumn-macros/src/repository.rs)
- Modify `#pg_name` struct fields to hold `statement_timeout: Option<Duration>`, `slow_query_threshold: Duration`, `matched_path: Option<String>`, and `metrics: MetricsCollector`.
- Populate these fields in the generated `FromRequestParts` implementation using the request extensions and `DbState`.
- Modify all macro-generated database queries in repository implementations to execute inside the instrumented telemetry wrapper.

---

### Component: Metrics & Actuator (`autumn/src/middleware/metrics.rs` & `autumn/src/actuator.rs`)

We will update `MetricsCollector` to track and report database query durations sharded by route and operation, and expose them inside `/actuator/metrics` and the Prometheus exporter `/actuator/prometheus`.

#### [MODIFY] [metrics.rs](file:///C:/Users/markm/.gemini/antigravity/worktrees/autumn/implement-issue-826-tdd/autumn/src/middleware/metrics.rs)
- Add database metrics tracking to `MetricsCollector` (tracking count, p50, p95, p99 per route + operation).
- Integrate database metrics in `snapshot() -> MetricsSnapshot`.

#### [MODIFY] [actuator.rs](file:///C:/Users/markm/.gemini/antigravity/worktrees/autumn/implement-issue-826-tdd/autumn/src/actuator.rs)
- Render database metrics in `/actuator/metrics`.
- Export database query duration and counter metrics in Prometheus format in `/actuator/prometheus`.

---

### Component: Error Handling (`autumn/src/error.rs`)

We will map Postgres SQLState `"57014"` to an RFC 7807 problem detail error.

#### [MODIFY] [error.rs](file:///C:/Users/markm/.gemini/antigravity/worktrees/autumn/implement-issue-826-tdd/autumn/src/error.rs)
- Add `AutumnError::query_timeout` constructor mapping Postgres statement cancellation to `503 Service Unavailable` with `autumn.query_timeout` machine code.

---

## Verification Plan

### Automated Tests
We will build a comprehensive integration test suite checking:
1. `parse_duration_str` unit tests for parsing correctness.
2. Global timeout execution: setting a `statement_timeout` triggers query cancellation when a query exceeds it.
3. Route-scoped timeout override: checking that a lower statement timeout override cancels long-running queries while the rest of the app uses defaults.
4. Telemetry assertions: asserting that slow queries log a parameter-scrubbed SQL fingerprint, matched route name, repository method name, trace ID, and query duration.
5. Metrics snapshot verification: verifying that `/actuator/metrics` contains sharded query counts and latency percentiles.
6. Pool resilience: verifying that timed-out connections are safely reset (`SET statement_timeout = 0` or new client timeout) and returned to the pool without parameter leakage or pool exhaustion.
