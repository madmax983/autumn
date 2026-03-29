# Chapter 10: Configuration and Production Defaults

**Goal:** By the end of this chapter, you will understand Autumn's profile-aware
configuration system, know how to override settings with environment variables,
and have a clear mental model for logging, health checks, and actuator
endpoints in development and production.

---

## Sections

### The Five Configuration Layers

1. Framework defaults
2. Profile smart defaults for `dev` / `prod`
3. `autumn.toml`
4. `autumn-{profile}.toml`
5. `AUTUMN_*` environment variables

How the layers merge, how the active profile is resolved, and why Autumn keeps
shared config separate from environment-specific overrides.

### `autumn.toml` Sections

Walking through every section: `[server]` (host, port, shutdown timeout),
`[database]` (URL, pool size, connect timeout), `[log]` (level, format),
`[health]` (path, detail level), `[actuator]` (sensitive endpoint exposure),
and the security/session sections that matter once you leave toy apps behind.

### Environment Variable Overrides

The `AUTUMN_SECTION__FIELD` naming convention (double underscore separates
sections). Examples: `AUTUMN_SERVER__PORT=8080`,
`AUTUMN_DATABASE__URL=postgres://...`, `AUTUMN_LOG__FORMAT=Json`,
`AUTUMN_PROFILE=prod`. Env vars always win over TOML layers.

### Structured Logging

The `LogFormat` enum: `Auto`, `Pretty`, `Json`. How profile smart defaults and
runtime environment interact, and when to force JSON for deployment logs.

### Health and Actuator

Auto-mounted `/health` for simple probes plus `/actuator/health`,
`/actuator/info`, `/actuator/metrics`, and the sensitive endpoints exposed in
development. Customizing the health path and reasoning about what should stay
visible in production.

### Graceful Shutdown

How `Ctrl+C` and `SIGTERM` trigger shutdown. The `shutdown_timeout_secs`
setting. Draining in-flight requests.

### Checkpoint

Expected project state with profile-aware config, environment overrides, and a
clear dev-vs-prod operational story.

---

*This chapter still needs the full walkthrough, but the outline above reflects
the current framework rather than the old pre-profile configuration model.*

---

Previous: [Chapter 9 — Error Handling](09-errors.md) | Next: [Chapter 11 — What's Next](11-whats-next.md)
