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

## Uniqueness and concurrency limits

`#[job]` can declare dedup and in-flight caps directly, so double-submits and
bursts cannot duplicate side effects or overwhelm downstream systems — no
hand-rolled advisory locks in job bodies.

```rust,ignore
// At most one identical sync in flight: a burst of N identical enqueues
// runs exactly once. The key defaults to a stable hash of the full args.
#[job(unique)]
async fn sync_search_index(state: AppState, args: SyncArgs) -> AutumnResult<()> { … }

// Key by selected args fields, and cap simultaneous executions per account.
#[job(unique_by = "account_id", concurrency = 1, concurrency_key = "account_id")]
async fn recalculate_account(state: AppState, args: RecalcArgs) -> AutumnResult<()> { … }

// Debounce: coalesce repeat enqueues for 60s from the first enqueue,
// even after the job completed.
#[job(unique_for_ms = 60_000)]
async fn rebuild_report(state: AppState, args: ReportArgs) -> AutumnResult<()> { … }
```

Attributes:

| Attribute | Meaning |
|---|---|
| `unique` | Dedupe on a stable hash of the full args payload. |
| `unique_by = "a, b"` | Dedupe on the listed args fields (implies `unique`). |
| `unique_window = "running"` | Default: key held while the job is pending **or** running; released when it settles. |
| `unique_window = "pending"` | Key released when execution starts, so a new instance may queue while one runs. |
| `unique_for_ms = N` | TTL window: key held for `N` ms from enqueue (and while in flight on Postgres), even past completion. Mutually exclusive with `unique_window`. |
| `concurrency = N` | At most `N` simultaneously-executing jobs of this type. |
| `concurrency_key = "field"` | Scope the limit per distinct value of this args field. |

Semantics:

- A coalesced enqueue is a **no-op `Ok(())`**; it is counted as
  `total_deduplicated` in `/actuator/jobs` and recorded with the
  `deduplicated` job-admin status.
- Jobs over the concurrency cap **wait** (they stay enqueued/parked and run
  when a slot frees) — they are never dropped.
- Keys and slots are released on success, terminal failure, **and worker
  crash**: Postgres ties them to row status recovered by the visibility
  timeout; Redis settles them in the claim-validated transition and
  stale-recovery scripts, with a TTL backstop on lock keys.
- Enforcement is **distributed-safe** across replicas on the durable
  backends: Postgres uses a partial unique index plus `ON CONFLICT DO
  NOTHING` for dedup and (only when a limited job is registered) a
  transaction-scoped advisory lock around claims; Redis uses `SET NX PX`
  locks and atomic Lua claim/settle scripts.
- With neither attribute set, behavior is unchanged: no dedup and unbounded
  per-type concurrency.
- Retries keep the unique key held (the job is still in flight) but release
  the concurrency slot while waiting out the backoff.
- The Postgres backend needs the additive `autumn migrate` migration that
  adds the nullable `unique_key`/`unique_window`/`concurrency_key`/
  `concurrency_limit` columns; rows and jobs without them behave as before.

## Observability

Mount `autumn-admin-plugin` to get the built-in operator dashboard at
`GET /admin/jobs` (or the plugin prefix you choose). It lists enqueued, running,
recently completed, and failed jobs with retry/discard/cancel actions. See the
[Operating Background Jobs](operating-background-jobs.md) guide for dashboard
setup, action semantics, and bounded refresh behavior.

`GET /actuator/jobs` returns per-job:

- `queued`
- `in_flight`
- `blocked_on_concurrency`
- `total_successes`
- `total_failures`
- `dead_letters`
- `total_deduplicated`
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

---

## Transactional enqueue

When a job must be coordinated with a database write, choose the API based on
which guarantee you need:

- `enqueue_after_commit` prevents jobs for rolled-back data on any backend, but
  the post-commit callback is process-local and can be lost if the process exits
  after commit.
- `enqueue_in_tx` / `enqueue_on_conn` on the Postgres backend write the job row
  in the same transaction as the domain row, which is the crash-safe handoff.

### `enqueue_after_commit` — any backend

`autumn_web::job::enqueue_after_commit` registers the enqueue as an
after-commit callback inside the surrounding `db.tx` block. The job is only
dispatched if the transaction commits. Works with every job backend.

This is not crash-safe delivery. If the process exits after the transaction
commits but before the callback runs, no job may be recorded. Use this for
rollback coordination across backends, not as a durable outbox substitute.

```rust,no_run
use autumn_web::prelude::*;
use scoped_futures::ScopedFutureExt;

async fn create_order(mut db: Db) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... INSERT order ...

        // Enqueued only after INSERT commits; dropped if the tx rolls back.
        // For crash-safe Postgres handoff, use enqueue_in_tx instead.
        autumn_web::job::enqueue_after_commit("ship_order", &args).await?;

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

### `enqueue_in_tx` / `enqueue_on_conn` — Postgres backend only

On the Postgres backend the job row can live in the **same transaction** as
the domain row. Both commit or roll back together, avoiding the post-commit
process crash window at the cost of being limited to the `postgres` backend.

```rust,no_run
use autumn_web::prelude::*;
use scoped_futures::ScopedFutureExt;

async fn create_order(mut db: Db) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... INSERT order using conn ...

        // Job row written into the same transaction.
        autumn_web::job::enqueue_in_tx("ship_order", &args, conn).await?;

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

See [Transactions -> after_commit](transactions.md#after_commit--post-commit-process-local-callbacks)
for a full comparison of the two strategies and guidance on when to use each.

For cloud-native rollout run the migration job first, then start web and workers.
