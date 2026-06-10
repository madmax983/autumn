# Actuator Endpoints Design

**Date:** 2026-03-26
**Status:** Validated (post six-hats review)
**Target:** v0.2.0

## Overview

Operational observability endpoints for monitoring, debugging, and managing running Autumn applications. Built-in, auto-mounted at `/actuator/*`, with profile-aware security defaults.

Replaces the existing standalone `/health` endpoint with a comprehensive actuator system.

## Endpoints

| Endpoint | Purpose | Sensitive | Dev | Prod |
|---|---|---|---|---|
| `/actuator/health` | App health, DB pool, scheduled task status | No | Enabled | Enabled |
| `/actuator/info` | Version, git commit, build time, profile, uptime | No | Enabled | Enabled |
| `/actuator/metrics` | Request count, latency, active connections, pool stats | No | Enabled | Enabled |
| `/actuator/env` | Active config values with redacted secrets | Yes | Enabled | Disabled |
| `/actuator/configprops` | All config properties with source tracking | Yes | Enabled | Disabled |
| `/actuator/loggers` | View and change log levels at runtime | Yes | Enabled | Disabled |
| `/actuator/logfile` | Recent structured log entries from in-memory ring buffer | Yes | Enabled | Disabled |
| `/actuator/tasks` | Active tokio tasks / scheduled task status | Yes | Enabled | Disabled |

## Configuration

```toml
[actuator]
prefix = "/actuator"      # default
sensitive = false          # default varies by profile
```

Profile-aware defaults:
- `dev`: `sensitive = true` (all endpoints enabled)
- `prod`: `sensitive = false` (only health, info, metrics)
- Override in TOML or env: `AUTUMN_ACTUATOR__SENSITIVE=true`

## Endpoint Details

### `/actuator/health`

Replaces existing `/health`. Reports application health, database pool status, and scheduled task health.

```json
{
    "status": "ok",
    "version": "0.1.0",
    "profile": "dev",
    "uptime": "2h 15m",
    "checks": {
        "database": {
            "status": "ok",
            "pool_size": 10,
            "active_connections": 3,
            "idle_connections": 7
        },
        "scheduled_tasks": {
            "status": "ok",
            "tasks": {
                "session-cleanup": {
                    "status": "ok",
                    "last_run": "2026-03-26T10:00:00Z",
                    "next_run": "2026-03-26T10:05:00Z"
                },
                "daily-digest": {
                    "status": "failed",
                    "last_run": "2026-03-26T00:00:00Z",
                    "last_error": "SMTP connection refused",
                    "next_run": "2026-03-27T00:00:00Z"
                }
            }
        }
    }
}
```

Top-level `status` is:
- `"ok"` — all checks pass
- `"degraded"` — some checks failing but app is functional
- `"down"` — critical check failing (e.g., database unreachable)

### `/actuator/info`

Static application metadata collected at build time.

```json
{
    "app": {
        "name": "my-blog",
        "version": "0.1.0"
    },
    "build": {
        "rust_version": "1.86.0",
        "profile": "debug",
        "timestamp": "2026-03-26T15:30:00Z"
    },
    "git": {
        "commit": "f52eb1f",
        "branch": "trunk",
        "dirty": false
    },
    "autumn": {
        "version": "0.2.0",
        "profile": "dev"
    },
    "runtime": {
        "uptime": "2h 15m",
        "started_at": "2026-03-26T13:15:00Z"
    }
}
```

Build and git info collected via environment variables:
- **Always available (no setup required):** `CARGO_PKG_NAME`, `CARGO_PKG_VERSION` (from Cargo), autumn framework version (from `autumn_web`'s own `CARGO_PKG_VERSION`)
- **Available with `vergen` (optional, graceful degradation):** `VERGEN_GIT_SHA`, `VERGEN_GIT_BRANCH`, `VERGEN_GIT_DIRTY`, `VERGEN_BUILD_TIMESTAMP`

If `vergen` is not configured, `/info` still returns app name, version, autumn version, and runtime info. Git and build timestamp fields are simply omitted rather than erroring. This ensures `/info` always works out of the box.

### `/actuator/metrics`

Runtime performance metrics. Uses `metrics` crate facade with an in-memory recorder.

```json
{
    "http": {
        "requests_total": 15423,
        "requests_active": 3,
        "latency_ms": {
            "p50": 12,
            "p95": 45,
            "p99": 120
        },
        "by_route": {
            "GET /posts": { "count": 8000, "p50_ms": 8, "p99_ms": 45 },
            "POST /posts": { "count": 2000, "p50_ms": 25, "p99_ms": 150 },
            "GET /actuator/health": { "count": 5423, "p50_ms": 1, "p99_ms": 3 }
        },
        "by_status": {
            "2xx": 14800,
            "4xx": 600,
            "5xx": 23
        }
    },
    "database": {
        "pool_size": 10,
        "active_connections": 3,
        "idle_connections": 7,
        "queries_total": 42000,
        "query_latency_ms": {
            "p50": 2,
            "p95": 8,
            "p99": 25
        }
    },
    "tasks": {
        "session-cleanup": {
            "runs_total": 288,
            "failures_total": 2,
            "avg_duration_ms": 150
        }
    }
}
```

Middleware automatically records:
- Request count, active connections, latency histogram (per route, per status code)
- Database pool checkout count, query latency (via instrumented pool wrapper)
- Scheduled task run count, failure count, duration

### `/actuator/env` (sensitive)

Active **Autumn configuration** values with secrets redacted. Scoped to `AutumnConfig` properties only — does NOT dump arbitrary system environment variables, limiting the blast radius of secret leakage.

```json
{
    "active_profile": "dev",
    "properties": {
        "server.host": "127.0.0.1",
        "server.port": 3000,
        "database.url": "****",
        "database.pool_size": 10,
        "log.level": "debug",
        "log.format": "pretty",
        "health.detailed": true,
        "actuator.prefix": "/actuator",
        "actuator.sensitive": true,
        "shutdown.drain_timeout": "1s"
    }
}
```

Redaction rules — values are replaced with `"****"` when the key contains:
- `password`, `secret`, `key`, `token`, `credential`, `auth`
- `url` (database URLs contain credentials)
- Any key listed in `[actuator] redact_keys`

### `/actuator/configprops` (sensitive)

All configuration properties with their source — which layer each value came from.

```json
{
    "active_profile": "dev",
    "properties": {
        "server.host": {
            "value": "127.0.0.1",
            "source": "profile_default:dev"
        },
        "server.port": {
            "value": 3000,
            "source": "autumn.toml"
        },
        "database.url": {
            "value": "****",
            "source": "autumn-dev.toml"
        },
        "database.pool_size": {
            "value": 10,
            "source": "autumn.toml"
        },
        "log.level": {
            "value": "debug",
            "source": "profile_default:dev"
        },
        "log.format": {
            "value": "pretty",
            "source": "AUTUMN_LOG__FORMAT"
        }
    }
}
```

Source values:
- `"default"` — hardcoded framework default
- `"profile_default:dev"` — smart default from active profile
- `"autumn.toml"` — base config file
- `"autumn-{profile}.toml"` — profile config file
- `"AUTUMN_{KEY}"` — environment variable

This is the debugging superpower — "why is this value what it is?" answered in one API call.

### `/actuator/loggers` (sensitive)

View current log levels and change them at runtime without redeploying.

**GET** `/actuator/loggers`:
```json
{
    "current_level": "debug",
    "available_levels": ["trace", "debug", "info", "warn", "error"],
    "loggers": {
        "autumn_web": "debug",
        "diesel": "info",
        "tower_http": "info",
        "hyper": "warn",
        "my_app": "debug"
    }
}
```

**PUT** `/actuator/loggers`:
```json
{
    "level": "trace",
    "logger": "diesel"
}
```

Response:
```json
{
    "status": "ok",
    "message": "Logger 'diesel' set to 'trace'",
    "previous": "info"
}
```

Implementation: `tracing-subscriber`'s `reload::Layer` creates a handle at startup. The PUT endpoint swaps the `EnvFilter` at runtime. Changes are ephemeral — they reset on restart. The current filter state is tracked in memory.

### `/actuator/tasks` (sensitive)

Active tokio tasks and scheduled task details. Useful for debugging "what's running right now?"

```json
{
    "scheduled_tasks": {
        "session-cleanup": {
            "schedule": "every 5m",
            "status": "idle",
            "last_run": "2026-03-26T10:00:00Z",
            "last_duration_ms": 150,
            "last_result": "ok",
            "next_run": "2026-03-26T10:05:00Z",
            "total_runs": 288,
            "total_failures": 2
        },
        "daily-digest": {
            "schedule": "cron 0 0 0 * * *",
            "status": "running",
            "started_at": "2026-03-26T00:00:00Z",
            "last_result": "failed",
            "last_error": "SMTP connection refused",
            "total_runs": 25,
            "total_failures": 3
        }
    },
    "tokio_runtime": {
        "active_tasks": 15,
        "worker_threads": 8,
        "blocking_threads": 2
    }
}
```

Tokio runtime stats available via `tokio::runtime::Handle::current().metrics()` (requires `tokio_unstable` cfg flag — may need to be optional or behind a feature flag).

## Dependencies

### New crates
- `vergen` — build-time git/rustc info for `/info` endpoint
- `metrics` + `metrics-util` — metrics facade and in-memory recorder for `/metrics`

### Existing crate integration
- `tracing-subscriber` `reload::Layer` — runtime log level changes for `/loggers`
- `deadpool` pool stats — connection pool metrics for `/health` and `/metrics`
- `tokio-cron-scheduler` — scheduled task status for `/health` and `/tasks`

### Integration with other v0.2 features
- **Profiles** — actuator security defaults are profile-aware (dev=open, prod=restricted)
- **`#[scheduled]`** — task status reported in `/health` and `/tasks`
- **Validation** — actuator config validated via same system
- **`#[repository]`** — database pool stats from the same pool repositories use

## Metrics Middleware

Auto-installed middleware that instruments every request:

```rust
// Automatically applied in AppBuilder::build_router()
pub struct MetricsLayer;

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsMiddleware<S>;
    // ...
}
```

**Critical: use Axum's matched route pattern, not the raw request path.** Axum gives you `/posts/{id}` (the pattern), not `/posts/42` (the path). This bounds cardinality to the number of routes, not the number of unique URLs. Requests that don't match any route (404s) are bucketed under `"_unmatched"`.

Records per request:
- `http_requests_total` (counter, labels: method, route_pattern, status)
- `http_requests_active` (gauge)
- `http_request_duration_ms` (histogram, labels: method, route_pattern)

Records per DB query (via instrumented pool):
- `db_queries_total` (counter)
- `db_query_duration_ms` (histogram)
- `db_pool_active` (gauge)
- `db_pool_idle` (gauge)

Records per scheduled task (via task wrapper):
- `task_runs_total` (counter, labels: task_name, result)
- `task_duration_ms` (histogram, labels: task_name)

## Spring Boot Comparison

| Spring Boot Actuator | Autumn Actuator |
|---|---|
| `/actuator/health` | `/actuator/health` |
| `/actuator/info` | `/actuator/info` (with git + build info) |
| `/actuator/metrics` | `/actuator/metrics` (with per-route breakdown) |
| `/actuator/env` | `/actuator/env` (with redaction) |
| `/actuator/configprops` | `/actuator/configprops` (with source tracking) |
| `/actuator/loggers` | `/actuator/loggers` (view + runtime change) |
| `/actuator/threaddump` | `/actuator/tasks` (tokio-native) |
| `management.endpoints.web.exposure.include` | `[actuator] sensitive = true/false` |
| Requires `spring-boot-starter-actuator` dep | Built-in, zero config |
| No profile-aware security defaults | Dev=open, prod=restricted |
| No config source tracking | Source tracking per property |

## Implementation Order

1. **Actuator config** — `[actuator]` section in config, profile-aware `sensitive` default
2. **Actuator router** — separate Axum router mounted at configurable prefix
3. **`/health` migration** — move existing health endpoint to `/actuator/health`, enhance with scheduled task status
4. **`/info` endpoint** — `build.rs` with `vergen` for git/build info, runtime uptime tracking
5. **Metrics infrastructure** — `metrics` crate recorder, `MetricsLayer` middleware
6. **`/metrics` endpoint** — serialize recorded metrics to JSON
7. **`/env` endpoint** — config value serialization with redaction
8. **`/configprops` endpoint** — config source tracking (requires config loader to record provenance)
9. **`/loggers` endpoint** — `reload::Layer` integration, GET + PUT handlers
10. **`/tasks` endpoint** — scheduled task registry with status tracking
11. **Tests** — endpoint response format, redaction, profile-aware access control
12. **Documentation** — actuator usage guide

## Security Model

### Profile defaults
- `dev`: all endpoints enabled (sensitive = true by default)
- `prod`: only health, info, metrics (sensitive = false by default)
- Custom profiles: sensitive = false by default

### Override in config
```toml
[actuator]
sensitive = true  # enable all endpoints regardless of profile
```

### Future: fine-grained control (v0.3+)
```toml
[actuator.endpoints]
health = true
info = true
metrics = true
env = false
configprops = false
loggers = true   # enable loggers but not env/configprops
tasks = false
```

### Future: auth middleware (v0.3+)
When Autumn adds auth/session support, actuator endpoints could require specific roles:
```toml
[actuator]
auth = "admin"  # require admin role for sensitive endpoints
```

## Redaction Rules

Values are redacted (replaced with `"****"`) in `/env` and `/configprops` when the flattened key contains any of:
- `password`
- `secret`
- `key` (but not `public_key` — refine later)
- `token`
- `credential`
- `auth`
- `url` (database URLs often contain credentials)

Additional redaction via config:
```toml
[actuator]
redact_keys = ["my_custom_secret", "api_endpoint"]
```

**Scope limitation:** `/env` and `/configprops` only expose `AutumnConfig` properties. They do NOT expose arbitrary system environment variables (`PATH`, `HOME`, `STRIPE_SK`, etc.). This prevents leaking secrets that don't follow Autumn's naming convention.

## Route Collision Detection

At startup, the actuator router checks if any user-defined route conflicts with actuator routes. If a collision is detected:

```
WARN user route "GET /actuator/health" conflicts with actuator endpoint — user route takes precedence
```

User routes always win. This is logged as a WARN so the user knows their route is shadowing an actuator endpoint.

## Risks & Mitigations (from Six Hats Review)

| Risk | Mitigation |
|---|---|
| Metrics cardinality explosion from path parameters (`/posts/1`, `/posts/2`, ...) | Use Axum's matched route pattern (`/posts/{id}`), not raw path; 404s bucketed as `"_unmatched"` |
| `/env` leaks secrets not matching redaction patterns | Scoped to `AutumnConfig` only — no arbitrary env vars; `redact_keys` config for custom additions |
| `/info` requires `vergen` build setup | Graceful degradation — always shows app version and runtime; git info is optional bonus |
| `/loggers` PUT is unauthenticated write access | Profile-aware: disabled in prod by default; dev-only risk is acceptable |
| Actuator routes conflict with user-defined routes | Collision detection at startup with WARN log; user routes take precedence |
| Metrics middleware adds latency under load | `metrics` crate uses atomic counters and pre-allocated storage; benchmark and document overhead |
| `/actuator/env` exposes all system env vars | Only Autumn config properties exposed, not system env vars |

## Future Enhancements

### Prometheus export (v0.3+)
Add `/actuator/metrics/prometheus` for Prometheus scraping alongside the JSON endpoint. The `metrics` crate has a `metrics-exporter-prometheus` backend that makes this straightforward.
