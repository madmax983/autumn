# Transactions

Use `Db::tx` when a handler must perform **multiple writes atomically**.

If every write in the closure succeeds, the transaction commits. If any step
returns `Err`, the transaction rolls back.

```rust,no_run
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use scoped_futures::ScopedFutureExt;

async fn create_two_rows(mut db: Db) -> AutumnResult<i64> {
    let id = db
        .tx(|conn| {
            async move {
                let id: i64 = diesel::insert_into(crate::schema::posts::table)
                    .values(crate::schema::posts::title.eq("hello"))
                    .returning(crate::schema::posts::id)
                    .get_result(conn)
                    .await?;

                diesel::insert_into(crate::schema::votes::table)
                    .values((
                        crate::schema::votes::post_id.eq(id),
                        crate::schema::votes::user_id.eq(1_i64),
                        crate::schema::votes::value.eq(1_i16),
                    ))
                    .execute(conn)
                    .await?;

                Ok::<_, AutumnError>(id)
            }
            .scope_boxed()
        })
        .await?;

    Ok(id)
}
```

## `db.tx` vs hooks

- Use repository hooks (`before_create`, `before_update`, `before_delete`) for
  model-local mutation concerns.
- Use `db.tx` when orchestration spans multiple writes and/or multiple tables in
  one route or service operation.

Hooks executed inside `db.tx` participate in the same database transaction.

## Panic and rollback

`Db::tx` delegates to Diesel async transaction handling. Operationally:

- `Ok(_)` commits
- `Err(_)` rolls back
- panics unwind through the transaction boundary and do not commit partial work

## Nesting policy

Nested `Db::tx` calls are currently **rejected at runtime** with:

`Nested Db::tx calls are not supported`

This avoids ambiguity and keeps transaction boundaries explicit.

---

## `after_commit` — post-commit process-local callbacks

### The dual-write problem

When a handler writes to the database **and** enqueues a job or sends an email,
there are two discrete operations:

- If the side effect runs before the DB commit and the transaction rolls back,
  the side effect fires against data that never existed.
- If the DB commits and the process exits before post-commit work runs, the
  side effect can still be lost.

`after_commit` callbacks solve the first problem only. They are closures
registered inside a `db.tx` block and spawned after the transaction commits
successfully. If the transaction rolls back, the callbacks are discarded.

They are **not a crash-safe delivery mechanism**. The callbacks are
process-local work handed to Tokio after the database commit has already
returned, so a process exit in that window can still lose a Redis enqueue,
external queue publish, email, or other side effect.

For crash-safe delivery, write a durable outbox, Postgres job row, or queue row
inside the same transaction as the domain write, then have a worker drain that
durable record. An `after_commit` callback may still be useful as a wake-up
hint, but the durable row must be the source of truth.

### `register_after_commit`

```rust,no_run
use autumn_web::db::register_after_commit;
use autumn_web::prelude::*;
use scoped_futures::ScopedFutureExt;

async fn create_user(mut db: Db) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... INSERT user ...

        // Registers a process-local closure to run AFTER the transaction
        // commits. If the transaction rolls back this closure is dropped.
        register_after_commit(|| async move {
            // Enqueue a job, call an external API, publish an event, etc.
            Ok(())
        })
        .await;

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

### Jobs — `enqueue_after_commit`

For the common cross-backend case of enqueueing a background job after a
successful write, use the free function `autumn_web::job::enqueue_after_commit`.
It behaves like `JobClient::enqueue` but defers the enqueue until after the
surrounding `db.tx` commits. Outside a transaction it enqueues immediately so it
is safe to call unconditionally.

This is still process-local deferral. If the process exits after commit but
before the callback runs, no job may be recorded. Use it when you need "no job
for rolled-back data"; use a transactional enqueue or durable outbox when the
job handoff itself must survive process loss.

```rust,no_run
use autumn_web::prelude::*;
use scoped_futures::ScopedFutureExt;

async fn publish_post(mut db: Db) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... INSERT post ...

        // Enqueued only if the INSERT commits -- no orphaned jobs.
        // Not crash-safe; use enqueue_in_tx for that on Postgres.
        autumn_web::job::enqueue_after_commit("post_publication", &args).await?;

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

When using the Postgres job backend, prefer `enqueue_in_tx` / `enqueue_on_conn`
for crash-safe job handoff. These APIs write the job row inside the same
database transaction, so the job row and domain row commit or roll back together
atomically. See [Jobs -> Transactional enqueue](jobs.md#transactional-enqueue).

### Mail — auto-deferred `deliver_later`

`Mailer::deliver_later` (and the `deliver_later_*` helpers generated by
`#[mailer]`) automatically detect when they are called inside a `db.tx`
block and defer mail dispatch until the transaction commits. No code change
is required — simply call `deliver_later` inside the closure.

Like any `after_commit` callback, this only prevents mail for rolled-back
writes. It does not make an in-process mail spawn, SMTP send, or external queue
handoff crash-safe unless the configured mail queue records a durable outbox row
or equivalent durable intent.

```rust,no_run
use autumn_web::prelude::*;
use scoped_futures::ScopedFutureExt;

async fn register_user(mut db: Db, mailer: Mailer) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... INSERT user ...

        // Automatically deferred until after commit, but not crash-safe by
        // itself.
        AccountMailer.deliver_later_welcome(&mailer, email, username);

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

To bypass deferral and spawn the mail task immediately regardless of
transaction state, call `deliver_later_eager` instead.

### Repository hooks

The repository macro wires the `after_create_commit`, `after_update_commit`,
and `after_delete_commit` hooks from `MutationHooks` when durable commit hooks
are explicitly enabled on the repository:

```rust,ignore
#[repository(Post, hooks = PostHooks, commit_hooks = true)]
pub trait PostRepository {}
```

Override them to run post-commit side effects without touching the generated
CRUD code:

```rust,no_run
impl MutationHooks for PostHooks {
    async fn after_create_commit(
        &self,
        ctx: &mut RequestContext,
        record: &Post,
    ) -> AutumnResult<()> {
        // Runs after the INSERT commits. Use a durable mail queue/outbox if the
        // notification itself must survive process exit.
        NotificationMailer.deliver_later_new_post(ctx.mailer(), record);
        Ok(())
    }
}
```

When a generated repository mutation runs inside an HTTP request covered by
Autumn idempotency, `MutationContext::idempotency_key` is populated with the
framework-scoped idempotency key. Durable `after_*_commit` queue rows use that
same scoped key to de-duplicate duplicate dispatch rows for a retried request,
and hook implementations can reuse it as a provider idempotency token for
external side effects.

### Observability

A process-level counter tracks failures in after-commit callbacks (for
example, a job broker being unreachable after the DB has already committed):

```rust,no_run
autumn_web::db::AFTER_COMMIT_FAILURES_TOTAL.load(std::sync::atomic::Ordering::Relaxed)
```

Scrape this counter in your metrics handler or dashboards. A non-zero value
means at least one committed transaction's side effect was not delivered and
may need manual recovery.

### When to use which approach

| Scenario | Recommended API |
|---|---|
| Job + DB write on any backend, avoiding rolled-back data | `enqueue_after_commit` inside `db.tx` |
| Crash-safe job + DB write on Postgres | `enqueue_in_tx` / `enqueue_on_conn` inside `db.tx` |
| Email triggered by a DB write, avoiding rolled-back data | `deliver_later` inside `db.tx` (auto-deferred) |
| Crash-safe email triggered by a DB write | Insert a durable outbox row in the transaction; use a mail queue/worker to drain it |
| Repository create/update/delete side effect | `after_create_commit` / `after_update_commit` / `after_delete_commit` hook with `commit_hooks = true` |
| Custom side effect on commit | `register_after_commit` inside `db.tx` |

## Bulk Repository Operations & Transactions

All generated bulk methods (`save_many`, `update_many`, `delete_many`, `upsert_many`) fully integrate with Autumn's transaction boundaries:

- **Atomic Execution**: On repositories with hooks configured, the entire batch query and hook execution are wrapped in an atomic database transaction. If any individual record hook fails or if the database returns an error, the entire operation is automatically rolled back.
- **Participation in `db.tx`**: If a bulk operation is called inside a `db.tx` block, it automatically participates in that outer transaction. No new nested transaction is started, conforming to Autumn's nesting policy.
- **Durable Commit Hooks**: If commit hooks are enabled (`commit_hooks = true`), post-commit hooks like `after_create_commit` will be staged during bulk writes and executed sequentially only when the surrounding database transaction successfully commits.

## Transactions and Read Replicas

When `database.replica_url` is configured, generated repository read methods normally route to the replica pool (see the [repositories guide](repositories.md#read-replicas-automatic-read-routing)). Transactions are the exception: **everything inside a transaction stays on the single primary connection that owns it**.

- `db.tx(|conn| ...)` hands your closure the transaction's primary connection; every query you run on `conn` — reads included — executes on the primary. There is no split-brain where a transaction writes to the primary but reads stale data from a replica.
- `repo.with_lock(id, |record, conn| ...)` performs its `SELECT ... FOR UPDATE` and runs your closure on a primary transaction connection, since locking reads are only meaningful on the writer.
- The internal transactions opened by generated write methods (`save`, `update`, bulk operations, hook lifecycles) acquire from the primary pool, so hook-driven reads-during-write also see the primary.

For read-your-writes *outside* a transaction — e.g. a handler that saves and then re-fetches — use the repository's `on_primary()` escape hatch instead of opening a transaction just to pin the connection.
