# Chapter 10: Configuration and Production Defaults

**Goal:** By the end of this chapter, you will understand Autumn's three-layer
configuration system, know how to override settings with environment
variables, and have configured structured logging and the health check
endpoint.

---

## Sections

### The Three Configuration Layers

1. Framework defaults (compiled in)
2. `autumn.toml` (project-level)
3. `AUTUMN_*` environment variables (deployment-level)

How the layers merge and why each exists.

### `autumn.toml` Sections

Walking through every section: `[server]` (host, port, shutdown timeout),
`[database]` (URL, pool size, connect timeout), `[log]` (level, format),
`[health]` (path).

### Environment Variable Overrides

The `AUTUMN_SECTION__FIELD` naming convention (double underscore separates
sections). Examples: `AUTUMN_SERVER__PORT=8080`,
`AUTUMN_DATABASE__URL=postgres://...`. Env vars always win over the TOML
file.

### Structured Logging

The `LogFormat` enum: `Auto`, `Pretty`, `Json`. Auto picks Pretty in
development and JSON in production (based on `AUTUMN_ENV`). Configuring log
levels with tracing filter syntax.

### The Health Check Endpoint

Auto-mounted at `/health` by default. Customizing the path. What to check in
a production health endpoint.

### Graceful Shutdown

How `Ctrl+C` and `SIGTERM` trigger shutdown. The `shutdown_timeout_secs`
setting. Draining in-flight requests.

### Checkpoint

Expected project state with production-ready configuration.

---

*Content coming in Sprint 12.*

---

Previous: [Chapter 9 — Error Handling](09-errors.md) | Next: [Chapter 11 — What's Next](11-whats-next.md)
