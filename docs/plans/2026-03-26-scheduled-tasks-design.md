# `#[scheduled]` Task Design

**Date:** 2026-03-26
**Status:** Validated (post six-hats review)
**Target:** v0.2.0

## Overview

A proc macro for declaring scheduled background tasks that run alongside your HTTP handlers in the same process. Built on `tokio-cron-scheduler`. Supports cron expressions and human-readable fixed-delay intervals.

## User-Facing API

### Declaring scheduled tasks

```rust
use autumn_web::{scheduled, AutumnResult};

// Fixed delay — runs every 5 minutes (after previous run completes)
#[scheduled(every = "5m", name = "session-cleanup")]
async fn cleanup_sessions(repo: PgSessionRepository) -> AutumnResult<()> {
    repo.delete_by_expired(true).await?;
    Ok(())
}

// Cron — runs at midnight every day (UTC by default)
#[scheduled(cron = "0 0 0 * * *", name = "daily-digest")]
async fn send_daily_digest(repo: PgUserRepository, state: AppState) -> AutumnResult<()> {
    let users = repo.find_by_digest_enabled(true).await?;
    // send emails...
    Ok(())
}

// Cron with explicit timezone
#[scheduled(cron = "0 0 0 * * *", tz = "America/New_York", name = "nightly-report")]
async fn nightly_report(state: AppState) -> AutumnResult<()> {
    // runs at midnight Eastern time
    Ok(())
}

// Compound duration
#[scheduled(every = "1h 30m", name = "cache-refresh")]
async fn refresh_cache(state: AppState) -> AutumnResult<()> {
    // refresh cached data...
    Ok(())
}
```

### Registering tasks with the application

```rust
use autumn_web::{get, routes, tasks, scheduled};

#[get("/")]
async fn index() -> &'static str { "Hello" }

#[scheduled(every = "5m", name = "session-cleanup")]
async fn cleanup_sessions(repo: PgSessionRepository) -> AutumnResult<()> {
    repo.delete_by_expired(true).await?;
    Ok(())
}

#[scheduled(cron = "0 0 0 * * *", name = "daily-digest")]
async fn send_daily_digest(repo: PgUserRepository) -> AutumnResult<()> {
    // ...
    Ok(())
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index])
        .tasks(tasks![cleanup_sessions, send_daily_digest])
        .run()
        .await;
}
```

## Design Decisions

### Built on `tokio-cron-scheduler`
Proven crate (719 stars, actively maintained), tokio-native, supports cron + intervals. Autumn wraps it with proc macro ergonomics and dependency injection. Same philosophy as Maud, Diesel, Axum — assemble proven crates, add the glue.

### Dependencies injected by type (same as handlers)
Scheduled tasks declare their dependencies as function parameters, just like HTTP handlers. The framework resolves them from `AppState` at invocation time. This composes directly with `#[repository]`:

```rust
#[scheduled(every = "5m")]
async fn cleanup(repo: PgSessionRepository) -> AutumnResult<()> {
    repo.delete_by_expired(true).await?;
    Ok(())
}
```

Supported parameter types:
- `AppState` — full application state
- `PgXxxRepository` — any generated repository (resolved from AppState's pool)
- `AutumnConfig` — application configuration

The macro generates a wrapper that extracts each parameter from `AppState`, matching the extractor pattern handlers use.

### `tasks![]` macro — parallel to `routes![]`
Explicit registration, consistent with existing patterns. A `#[scheduled]` function generates a companion `__autumn_task_info_{name}()` function (same pattern as route macros), and `tasks![]` collects them.

### Fixed delay, not fixed rate
`every = "5m"` means "wait 5 minutes after the previous run completes." Tasks never overlap. This is safer than fixed rate (where slow tasks pile up) and matches what most people actually want.

### Cron expressions
Standard 6-field cron (with seconds): `sec min hour day month weekday`. Parsed by `tokio-cron-scheduler` which uses the `cron` crate internally. **Default timezone is UTC.** Optional `tz` parameter for explicit timezone:

```rust
#[scheduled(cron = "0 0 0 * * *", tz = "America/New_York")]
```

Valid timezone values are IANA timezone names. Invalid timezone is a compile error.

### Fail-fast dependency resolution
When `.run()` is called, all task dependencies are resolved **once at startup** before the scheduler begins. If a task requires `PgSessionRepository` but no database pool is configured, the application panics immediately with a clear message:

```
PANIC: scheduled task "session-cleanup" requires a database pool, but no [database] config was found.
```

This prevents the scenario where a task silently fails on first invocation minutes or hours after startup.

### Human-readable duration syntax
`every` accepts duration strings parsed at compile time:
- `"5s"` — 5 seconds
- `"5m"` — 5 minutes
- `"1h"` — 1 hour
- `"1h 30m"` — 1 hour 30 minutes
- `"1d"` — 1 day

Invalid durations are compile errors: `"5x"` → `"Invalid duration unit 'x'. Valid units: s, m, h, d"`

### Named tasks for observability
Every scheduled task has a `name` used in:
- Log messages: `INFO scheduled task "session-cleanup" started`
- Error logs: `WARN scheduled task "session-cleanup" failed: connection refused`
- Future: health check endpoint could report task status

If `name` is omitted, defaults to the function name.

### Error handling: log and continue
If a task returns `Err`, it is logged at WARN level and the task runs again at its next scheduled time. No retry, no circuit breaker in v0.2. Designed to add `retries` and `backoff` attributes later without breaking changes.

```
WARN scheduled task "session-cleanup" failed: database connection refused
INFO scheduled task "session-cleanup" next run in 5m
```

### Scheduler heartbeat
The scheduler registers a heartbeat that tracks its last tick time. If the scheduler hasn't ticked in 2x the shortest task interval, `/actuator/health` reports it as unhealthy:

```json
{
    "checks": {
        "scheduler": {
            "status": "down",
            "last_tick": "2026-03-26T08:00:00Z",
            "expected_interval": "5m",
            "message": "Scheduler has not ticked in 10m — may have crashed"
        }
    }
}
```

This catches the silent failure case where the scheduler background task panics and no tasks run, but the HTTP server remains healthy.

## Generated Code

Given:
```rust
#[scheduled(every = "5m", name = "session-cleanup")]
async fn cleanup_sessions(repo: PgSessionRepository) -> AutumnResult<()> {
    repo.delete_by_expired(true).await?;
    Ok(())
}
```

The macro generates:

### Task info companion function
```rust
fn __autumn_task_info_cleanup_sessions() -> autumn_web::task::TaskInfo {
    autumn_web::task::TaskInfo {
        name: "session-cleanup".to_string(),
        schedule: autumn_web::task::Schedule::FixedDelay(std::time::Duration::from_secs(300)),
        handler: |state: AppState| Box::pin(async move {
            // Resolve dependencies from AppState
            let pool = state.pool()
                .ok_or_else(|| AutumnError::internal("No database pool configured"))?
                .clone();
            let repo = PgSessionRepository { pool };

            // Call the user's function
            cleanup_sessions(repo).await
        }),
    }
}
```

### TaskInfo type
```rust
pub mod task {
    pub struct TaskInfo {
        pub name: String,
        pub schedule: Schedule,
        pub handler: fn(AppState) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>>,
    }

    pub enum Schedule {
        FixedDelay(std::time::Duration),
        Cron { expression: String, timezone: Option<String> },
    }
}
```

### `tasks![]` macro
```rust
macro_rules! tasks {
    ($($task:ident),* $(,)?) => {
        vec![$(__autumn_task_info_$task()),*]
    };
}
```

### AppBuilder integration
```rust
impl AppBuilder {
    pub fn tasks(mut self, tasks: Vec<TaskInfo>) -> Self {
        self.tasks = tasks;
        self
    }
}
```

When `.run()` is called, the app builder:
1. Creates a `JobScheduler` from `tokio-cron-scheduler`
2. **Fail-fast: resolves all task dependencies from AppState** — panics if any are missing
3. For each `TaskInfo`, creates a `Job` with the appropriate schedule
4. Wraps each job's handler to:
   - Log task start at INFO level (including timezone for cron tasks)
   - Call the handler with a clone of `AppState`
   - Log success at INFO level or failure at WARN level
   - For fixed delay: schedule next run after completion
5. Registers a **scheduler heartbeat** — tracks last tick time for health check integration
6. Starts the scheduler alongside the Axum server
7. On graceful shutdown: stops the scheduler, waits for in-flight tasks

## Lifecycle & Shutdown

```
app.run() called
  ├─ Start Axum HTTP server
  ├─ Start tokio-cron-scheduler
  │   ├─ Register all tasks from tasks![]
  │   └─ Begin scheduling
  └─ Wait for shutdown signal (Ctrl+C / SIGTERM)
       ├─ Stop accepting new HTTP connections
       ├─ Stop scheduling new task runs
       ├─ Wait for in-flight tasks (with drain timeout from config)
       └─ Exit
```

Scheduled tasks respect the same `shutdown.drain_timeout` config as HTTP handlers.

## Full Example

```rust
use autumn_web::{get, post, routes, tasks, scheduled, Json, Valid, AutumnResult, AppState};
use crate::models::{Post, NewPost, Session};
use crate::repositories::{PostRepository, PgPostRepository, PgSessionRepository};

// --- Handlers ---
#[get("/posts")]
async fn list(repo: PgPostRepository) -> AutumnResult<Json<Vec<Post>>> {
    Ok(Json(repo.find_by_published(true).await?))
}

#[post("/posts")]
async fn create(
    repo: PgPostRepository,
    Valid(Json(new)): Valid<Json<NewPost>>
) -> AutumnResult<Json<Post>> {
    Ok(Json(repo.save(&new).await?))
}

// --- Scheduled Tasks ---
#[scheduled(every = "5m", name = "session-cleanup")]
async fn cleanup_sessions(repo: PgSessionRepository) -> AutumnResult<()> {
    repo.delete_by_expired(true).await?;
    tracing::info!("cleaned up expired sessions");
    Ok(())
}

#[scheduled(cron = "0 0 2 * * *", name = "nightly-optimization")]
async fn optimize_db(state: AppState) -> AutumnResult<()> {
    // run VACUUM ANALYZE or similar
    Ok(())
}

// --- Bootstrap ---
#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![list, create])
        .tasks(tasks![cleanup_sessions, optimize_db])
        .run()
        .await;
}
```

## Spring Boot Comparison

| Spring Boot | Autumn |
|---|---|
| `@EnableScheduling` | `.tasks(tasks![...])` on app builder |
| `@Scheduled(fixedDelay = 300000)` | `#[scheduled(every = "5m")]` |
| `@Scheduled(cron = "0 0 * * * *")` | `#[scheduled(cron = "0 0 * * * *")]` |
| `@Autowired` dependencies | Function parameters (injected from AppState) |
| Runtime exception → silent failure | `AutumnResult<()>` → logged at WARN |
| `TaskScheduler` bean | `tokio-cron-scheduler` under the hood |
| Millisecond duration literals | Human-readable `"5m"`, `"1h 30m"` |
| `@Async` + thread pool | Tokio tasks (async by default) |

## Implementation Order

1. **Add `tokio-cron-scheduler` dependency** — workspace dep
2. **Duration parser** — compile-time parser for `"5m"`, `"1h 30m"` syntax
3. **`TaskInfo` type + `Schedule` enum** — core types in `autumn::task` module
4. **`#[scheduled]` macro** — generates companion function with dependency resolution
5. **`tasks![]` macro** — collects task info, parallel to `routes![]`
6. **`AppBuilder::tasks()`** — registers tasks, starts scheduler in `.run()`
7. **Graceful shutdown integration** — stop scheduler on SIGTERM, drain in-flight tasks
8. **Logging wrapper** — INFO on start/success, WARN on failure, task name in all messages
9. **Tests** — duration parser unit tests, task registration tests, lifecycle tests
10. **Example** — add scheduled task to todo-app or blog example

## Risks & Mitigations (from Six Hats Review)

| Risk | Mitigation |
|---|---|
| Scheduler background task panics silently, tasks stop running | Scheduler heartbeat tracked in health check — detects dead scheduler |
| Task dependencies missing at runtime (e.g., no DB pool) | Fail-fast: resolve all dependencies at startup, panic with clear message |
| Timezone confusion — cron defaults to UTC | Log next run time with timezone at startup; optional `tz` parameter for explicit timezone |
| Long-running tasks block DB pool, starve HTTP handlers | Shared pool is documented; users advised to use short-lived connections and consider dedicated pool for heavy tasks (v0.3+) |
| In-flight task blocks graceful shutdown | Tasks respect `shutdown.drain_timeout`; task is cancelled if timeout expires |

## Future Enhancements (v0.3+)

### Retry with backoff
```rust
#[scheduled(every = "5m", retries = 3, backoff = "30s")]
async fn sync_external(state: AppState) -> AutumnResult<()> { ... }
```

### Error hooks
```rust
#[scheduled(every = "5m", on_error = "alert_ops")]
async fn critical_task(state: AppState) -> AutumnResult<()> { ... }
```

### Task status in health endpoint
```json
{
    "status": "ok",
    "tasks": {
        "session-cleanup": { "last_run": "2026-03-26T10:00:00Z", "status": "ok" },
        "daily-digest": { "last_run": "2026-03-26T00:00:00Z", "status": "failed", "error": "SMTP timeout" }
    }
}
```

### Distributed locking
For multi-instance deployments, only one instance should run each scheduled task. Postgres advisory locks via the database pool Autumn already manages.

## Dependencies

### Integration with `#[repository]`
Scheduled tasks use the same `PgXxxRepository` types as handlers. The macro resolves repositories from `AppState`'s pool, identical to the extractor path.

### Integration with `AppState`
The scheduler holds a clone of `AppState` and passes it to each task invocation. Same lifecycle as the HTTP server.

### Integration with graceful shutdown
Scheduler shutdown is wired into the same signal handler as the HTTP server. The `shutdown.drain_timeout` config applies to both in-flight HTTP requests and in-flight scheduled tasks.
