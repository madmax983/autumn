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
5. SMTP-backed mail if the app sends account or notification email
6. an explicit migration job before web replicas start
7. a clear choice between `#[scheduled]` and Harvest for background work

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

### What you get automatically

With the `telemetry-otlp` cargo feature enabled and `telemetry.enabled = true`:

- **W3C Trace Context propagation.** Incoming `traceparent` / `tracestate`
  headers are extracted and attached to a server span; the current context
  is injected back into the response headers so callers can continue the
  trace. No manual middleware setup required.
- **Scheduled task traces.** Each invocation of a `#[scheduled]` function
  runs inside a fresh root span (`scheduled_task` / `task=<name>`) so
  every run shows up as its own trace in your APM.
- **Database spans.** The `Db` extractor opens a `db.connection.acquire`
  span tagged with `db.system=postgresql` whose scope covers the lifetime
  of the pooled connection — Diesel activity performed through it appears
  as a child of the request span in Jaeger / Tempo / Datadog.

## File Uploads

The `Multipart` extractor's `save_to(path)` primitive writes to the local
disk of whichever pod handled the request — invisible to the next replica
and gone on the next deploy. For multi-replica deployments that accept
user-uploaded files (avatars, attachments, generated reports), enable the
`storage` feature and pick the `S3` backend:

```toml
[storage]
backend = "s3"

[storage.s3]
bucket = "my-app-uploads"
region = "us-east-1"
```

In `prod`, `backend = "local"` fails fast at startup unless you set
`storage.allow_local_in_production = true` — same pattern as the session
backend's memory-in-prod check.

See [storage.md](storage.md) for the full backend, configuration, and
production-checklist story.

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

## Mail

The `mail` cargo feature gives apps a `Mailer` extractor and log/file/SMTP
transports. Development defaults to `transport = "log"` so password-reset and
signup flows can be built before SMTP exists. Production rejects log transport
unless `mail.allow_log_in_production = true` is set.

Use SMTP in production:

```toml
[mail]
transport = "smtp"
from = "Acme <noreply@example.com>"

[mail.smtp]
host = "smtp.example.com"
port = 587
username = "apikey"
password_env = "SMTP_PASSWORD"
tls = "starttls"
```

For durable retries across replicas, register a durable
[`MailDeliveryQueue`](mail.md#deferred-delivery-deliver_later) via
`AppBuilder::with_mail_delivery_queue` before `.run()` (see the Mail Guide
for the trait definition and an outbox example). Without one, `prod` startup
fails unless you explicitly set
`mail.allow_in_process_deliver_later_in_production = true`, which
acknowledges the in-process Tokio fallback. The fallback is fine for local
development and small single-process deployments but is not durable across
restarts or replicas.

When email dispatch is coordinated with DB writes, use
[`Db::tx`](transactions.md) for the database side so the write set commits or
rolls back atomically.

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

## Shared Cache

In-process Moka caches are the zero-config default and are perfect for
local development. Each replica holds its own independent store, so:

- `#[cached]` may return stale data depending on which replica answers.
- `CacheResponseLayer` invalidations on one pod are invisible to peers.

For multi-replica production deployments, enable the Redis backend via
`autumn-cache-redis`:

```toml
# autumn.toml
[cache]
backend = "redis"

[cache.redis]
url = "redis://redis:6379"
key_prefix = "myapp:cache"
```

```rust
// main.rs
use autumn_cache_redis::RedisCachePlugin;

autumn_web::app()
    .plugin(RedisCachePlugin::new())
    .routes(routes![...])
    .run()
    .await;
```

`autumn-cache-redis` requires the `autumn-cache-redis` crate:

```toml
# Cargo.toml
[dependencies]
autumn-cache-redis = "0.3"
```

`CacheResponseLayer::from_app(&state)` returns `Some(layer)` wired to the
configured Redis backend when one has been registered, or `None` when running
with the default per-function Moka caches.

The `memory` default produces a startup warning in the `prod` profile — the
same pattern as sessions and file storage.

## Concurrent Writes

### The lost-update problem

With more than one replica, two requests can read the same row, compute
independent changes, and both write back — the second write silently overwrites
the first. No error is raised, no conflict is detected, data is lost.

### Optimistic concurrency via `#[lock_version]`

Add the attribute to any model field named `lock_version`:

```rust
#[autumn_web::model]
pub struct Article {
    #[id]
    pub id: i64,
    pub title: String,
    pub body: String,
    #[lock_version]
    pub lock_version: i32,
    #[default]
    pub created_at: chrono::NaiveDateTime,
    #[default]
    pub updated_at: chrono::NaiveDateTime,
}
```

The framework then:

1. Stores the current `lock_version` as a counter column in the database.
2. Requires the client to send the expected version alongside its update
   payload (the generated `UpdateArticle` struct carries this automatically).
3. On write, issues an atomic `UPDATE … WHERE id = $1 AND lock_version = $2`
   and increments the counter only if the row matched.

If the row was updated between the client's read and its write — i.e. the
stored version is no longer what the client expected — the UPDATE matches zero
rows and the repository returns `RepositoryError::Conflict`, which the
framework maps to HTTP 409 with an RFC 7807 problem body:

```json
{"type": "about:blank", "status": 409, "title": "Conflict", "detail": "..."}
```

For htmx clients, the framework also emits the response header:

```
HX-Trigger: {"autumn:conflict":true}
```

Your client-side script can listen for that event and re-fetch the current
version before letting the user resubmit.

A handler that catches the conflict and signals a retry:

```rust
async fn update_article(
    State(repo): State<ArticleRepository>,
    Path(id): Path<i64>,
    Form(input): Form<UpdateArticle>,
) -> Response {
    match repo.update(id, input).await {
        Ok(article) => Redirect::to(&format!("/articles/{}", article.id)).into_response(),
        Err(RepositoryError::Conflict { .. }) => {
            // Re-fetch the current version and tell the user to retry
            let current = repo.find(id).await.unwrap();
            (StatusCode::CONFLICT, EditTemplate { article: current, conflict: true })
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}
```

Optimistic concurrency is the right default for most web applications: reads
are cheap, conflicts are rare, and throughput scales well.

### Pessimistic concurrency via `with_lock`

For low-contention but high-consequence writes — think inventory deductions,
financial ledger entries, or seat reservations — you cannot afford to retry
after detecting a conflict because another request may have already acted on
the same data. Use `with_lock` to acquire a database-level advisory or
row-level lock before reading:

```rust
repo.with_lock(id, |row, conn| async move {
    // `row` is the freshly locked Page; `conn` is the transaction connection.
    // Any writes here are serialized against other `with_lock` callers for
    // the same `id`.
    row.seats_remaining -= 1;
    diesel::update(seats::table.find(row.id))
        .set(seats::seats_remaining.eq(row.seats_remaining))
        .execute(conn)
        .await?;
    Ok(row)
}.scope_boxed()).await
```

The closure runs inside a transaction. The lock is released when the
transaction commits or rolls back.

### Trade-offs

| | Optimistic | Pessimistic |
|---|---|---|
| **Throughput** | High — no blocking between readers | Lower — concurrent writers queue |
| **Latency** | Low on the happy path; a retry adds one round trip | Consistently higher; each writer waits for the lock |
| **Complexity** | Low — framework handles version checks | Moderate — closure-based API, must reason about deadlocks |
| **Best for** | Typical CRUD, forms, wiki edits, profile updates | Inventory, ledger, seat/slot reservation, anything where retry is unsafe |

### Cache invalidation after writes

`after_update` hooks receive a `MutationContext` that accepts
`ctx.invalidate("key")` calls to declare cache keys that should be evicted
after the write commits. This is coordinated with the shared-cache integration
(#535) so that the correct backend — Moka or Redis — is targeted regardless of
which replica processed the write:

```rust
async fn after_update(&self, ctx: &mut MutationContext, page: &Page) -> AutumnResult<()> {
    ctx.invalidate(format!("pages:{}", page.id));
    ctx.invalidate("pages:all");
    Ok(())
}
```

## Minimal Deployment Checklist

Before calling an Autumn app "cloud ready", verify:

- probes target `/live`, `/ready`, and `/startup`
- logs or traces land in your collector
- sessions are externalized if replicas > 1
- cache uses the Redis backend if replicas > 1
- file uploads use the `S3` blob store if replicas > 1
- mail uses SMTP, not log/file transport
- migrations run before web rollout
- background jobs use the right runtime model
- multi-replica write paths use `#[lock_version]` (optimistic) or `with_lock` (pessimistic) to prevent lost updates
- the generated container image builds without manual template surgery
