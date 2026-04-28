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
backend = "local"   # local | redis
workers = 2
max_attempts = 5
initial_backoff_ms = 250

[jobs.redis]
url = "redis://127.0.0.1/"
key_prefix = "autumn:jobs"
```

- `local`: in-process queue, zero configuration.
- `redis`: durable queue for multi-replica workers.

## Retry/backoff and dead letters

- Jobs retry with exponential backoff (`initial_backoff_ms * 2^(attempt-1)`).
- Retries stop at `max_attempts` (job-level override or config default).
- Exhausted jobs are dead-lettered.

## Observability

`GET /actuator/jobs` returns per-job:

- `queued`
- `in_flight`
- `total_successes`
- `total_failures`
- `dead_letters`
- `last_error`

## Migration notes

When using `jobs.backend = "redis"`, no SQL migration is required.

For cloud-native rollout, run your app migrations as a one-shot `autumn migrate` job before scaling web and workers.
