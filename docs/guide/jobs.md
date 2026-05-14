# Background Jobs (`#[job]`)

Autumn provides first-class ad-hoc background jobs for request-triggered async work.

## Define a job

```rust,ignore
use autumn_web::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelcomeEmailArgs {
    pub user_id: i64,
}

#[job(name = "send_welcome_email", max_attempts = 6, backoff_ms = 500)]
async fn send_welcome_email(state: AppState, args: WelcomeEmailArgs) -> AutumnResult<()> {
    // perform async side effect
    Ok(())
}
```

## Register jobs

```rust,ignore
autumn_web::app()
    .routes(routes![signup])
    .jobs(jobs![send_welcome_email])
    .run()
    .await;
```

## Enqueue from handlers

```rust,ignore
SendWelcomeEmailJob::enqueue(WelcomeEmailArgs { user_id: 42 }).await?;
```

## Backend selection (`autumn.toml`)

```toml
[jobs]
backend = "local"   # local | postgres | redis
workers = 2
max_attempts = 5
initial_backoff_ms = 250

[jobs.postgres]
# Reuses the configured [database] pool. No extra URL needed.
visibility_timeout_ms = 30000   # default: 30 000 ms

[jobs.redis]
url = "redis://127.0.0.1/"
key_prefix = "autumn:jobs"
visibility_timeout_ms = 30000
```

| Backend | Durable | Multi-replica safe | Extra infra |
|---|---|---|---|
| `local` | No | No (in-process) | None |
| `postgres` | Yes | Yes (SKIP LOCKED) | DB only — no Redis |
| `redis` | Yes | Yes | Redis |

- `local`: in-process channel, zero configuration. Jobs are lost on restart. Fine
  for development or single-process demos.
- `postgres`: Postgres-backed queue that reuses your existing `[database]` pool.
  Jobs survive restarts and are claimed atomically across replicas via
  `SELECT … FOR UPDATE SKIP LOCKED`. Requires the `db` feature and an
  `autumn migrate` run before the first worker starts.
- `redis`: Durable, Redis-backed queue for multi-replica workers. Higher
  throughput ceiling than `postgres` but adds Redis as an infrastructure dependency.

## Postgres delivery semantics

The Postgres backend provides **at-least-once delivery**. Each job is a row in
the `autumn_jobs` table. Workers claim a row atomically with
`UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED)`, which prevents any
two replicas from claiming the same job simultaneously.

A claimed job's status is set to `running` with a `claimed_at` timestamp and a
`claimed_by` worker id. A maintenance loop running inside each worker process
requeues jobs whose `claimed_at` is older than `jobs.postgres.visibility_timeout_ms`.
Recovered stale claims consume another attempt and record a `last_error`
explaining the visibility timeout.

If a job exhausts `max_attempts`, its status is set to `failed`; it is no longer
retried.

Because the backend provides at-least-once delivery, handlers must be idempotent.
A slow worker that outlives the visibility timeout can overlap with a recovered
retry, so external side effects should use natural idempotency keys such as the
job id, a domain aggregate id, or a provider idempotency token.

## Redis delivery semantics

The Redis backend provides **at-least-once delivery**. A job is written as a
durable record, queued by id, atomically claimed into an in-flight set, and
acked only after the handler returns `Ok(())`.

If a worker crashes after claiming a job, the record remains in Redis. Another
worker requeues the stale claim after `jobs.redis.visibility_timeout_ms`.
Recovered stale claims consume another attempt and retain a `last_error`
explaining the visibility timeout. If the job has exhausted `max_attempts`, it
is moved to the dead-letter list instead of being requeued.

Because Redis uses at-least-once delivery, handlers must be idempotent. A worker
that is slow beyond the visibility timeout can overlap with a recovered retry,
so external side effects should use natural idempotency keys such as the job id,
domain aggregate id, or provider idempotency token.

## Retry/backoff and dead letters

- Jobs retry with exponential backoff (`initial_backoff_ms * 2^(attempt-1)`).
- Retries stop at `max_attempts` (job-level override or config default).
- Exhausted jobs are dead-lettered.
- Redis retries are scheduled in Redis before the worker moves on, so a crash
  during the backoff window does not drop the job.

## Observability

Mount `autumn-admin-plugin` to get the built-in operator dashboard at
`GET /admin/jobs` (or the plugin prefix you choose). It lists enqueued, running,
recently completed, and failed jobs with retry/discard/cancel actions. See the
[Operating Background Jobs](operating-background-jobs.md) guide for dashboard
setup, action semantics, and bounded refresh behavior.

`GET /actuator/jobs` returns per-job:

- `queued`
- `in_flight`
- `total_successes`
- `total_failures`
- `dead_letters`
- `last_error`

For Redis deployments these counters are process-local operational telemetry,
not a strongly consistent Redis aggregate. They remain useful for seeing queued,
in-flight, success, retry/failure, and dead-letter activity observed by the
replica serving the actuator request.

## Migration notes

When using `jobs.backend = "local"` or `jobs.backend = "redis"`, no SQL migration
is required.

When using `jobs.backend = "postgres"`, the `autumn_jobs` table must exist before
workers start. Run your app migrations as a one-shot `autumn migrate` job before
scaling web and worker replicas:

```bash
autumn migrate   # creates autumn_jobs, your domain tables, etc.
```

The migration is bundled with the framework and is applied automatically by
`autumn migrate` as long as the `db` feature is enabled.

For cloud-native rollout run the migration job first, then start web and workers.
