# Cloud-Native Autumn

This guide is the blunt version: Autumn can run as a single-process monolith
with almost no config, but that does not automatically make every default safe
for multi-replica production.

## Baseline

A production-minded Autumn deployment should usually have all of these in
place:

1. `AUTUMN_PROFILE=prod`
2. `/live`, `/ready`, and `/startup` connected to platform probes
3. OTLP telemetry enabled, or at minimum JSON logs
4. Redis-backed sessions if more than one web replica will serve the same users
5. an explicit migration job before web replicas start
6. a clear choice between `#[scheduled]` and Harvest for background work

## What `autumn new` Gives You

The scaffold now includes:

- `Dockerfile` with a multi-stage build
- `.dockerignore`
- commented probe, telemetry, and Redis session examples in `autumn.toml`

That is container scaffolding, not a full cluster deployment. You still need to
decide your runtime topology.

## Probes

Autumn mounts:

- `/live`
- `/ready`
- `/startup`
- `/health`

Recommended use:

- liveness probe -> `/live`
- readiness probe -> `/ready`
- startup probe -> `/startup`

Do not point all three at `/health` just because it was easy in older apps.

## Telemetry

Use the `[telemetry]` section or the `AUTUMN_TELEMETRY__*` environment
variables to declare service metadata and an OTLP endpoint.

Example:

```toml
[telemetry]
enabled = true
service_name = "bookmarks"
service_namespace = "apps"
environment = "production"
otlp_endpoint = "http://otel-collector:4317"
protocol = "Grpc"
```

If you are not ready for OTLP yet, force `log.format = "Json"` so your logs are
at least machine-readable.

## Sessions

In-memory sessions are fine for local development and single-process demos.
They are the wrong default for horizontally scaled apps.

Use:

```toml
[session]
backend = "redis"

[session.redis]
url = "redis://redis:6379"
key_prefix = "my-app:sessions"
```

The `prod` profile warns when you keep `backend = "memory"` without explicitly
acknowledging it via `session.allow_memory_in_production = true`.

## Background Work

Use `#[scheduled]` when:

- the task is small and in-process
- duplicate execution per replica is acceptable or explicitly coordinated
- you do not need durable retries or workflow history

Use Harvest when:

- the task must survive restarts
- retries and visibility matter
- work should be coordinated across replicas
- you are really describing a workflow, not a cron callback

## Migration Jobs

For multi-replica deployments, do not rely on each web process racing to apply
migrations. Run migrations once as a dedicated job, then start the web
deployment after it succeeds.

The distributed bookmarks example uses this shape explicitly.

## Minimal Deployment Checklist

Before calling an Autumn app "cloud ready", verify:

- probes target `/live`, `/ready`, and `/startup`
- logs or traces land in your collector
- sessions are externalized if replicas > 1
- migrations run before web rollout
- background jobs use the right runtime model
- the generated container image builds without manual template surgery
