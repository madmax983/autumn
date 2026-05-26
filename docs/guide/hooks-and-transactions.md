# Hooks & Transactions

This guide covers every layer of Autumn's mutation pipeline: database transactions,
repository lifecycle hooks, post-commit callbacks, and bulk operations. Each layer
has a distinct scope and different error semantics. Understanding how they compose
is the key to writing correct, side-effect-safe code.

---

## Mental model

Autumn gives you three concentric boundaries around a mutation:

```
┌─────────────────── db.tx ───────────────────────────────────┐
│                                                              │
│  ┌────────────── repository mutation ───────────────────┐   │
│  │                                                       │   │
│  │  before_* hook → SQL → after_* hook                  │   │
│  │                                                       │   │
│  └───────────────────────────────────────────────────────┘   │
│                                                              │
│  register_after_commit / enqueue_after_commit                │
│  deliver_later (auto-deferred)                               │
│                                                              │
└──────────────────────────────────────────────────────────────┘
           │
           ▼ (after Postgres confirms COMMIT)
  after_*_commit hook  (durable, crash-safe — requires commit_hooks = true)
```

- **`db.tx`** — wraps any number of SQL statements (or repository calls) in a
  single Postgres transaction.
- **Repository hooks** (`before_*`, `after_*`) — run inside whichever transaction
  is active when the repository method is called.
- **`after_commit` callbacks** — registered inside a transaction, run after the
  transaction durably commits. Process-local; not crash-safe by themselves.
- **`after_*_commit` hooks** — durable, Postgres-backed, survive process crashes.
  Opt-in per repository.

---

## `db.tx` — explicit atomic boundaries

Use `db.tx` when a handler must write to multiple tables and all writes must
succeed or fail together.

```rust,no_run
use autumn_web::prelude::*;
use scoped_futures::ScopedFutureExt;

async fn accept_order(mut db: Db, order_id: i64) -> AutumnResult<()> {
    db.tx(|conn| {
        async move {
            diesel::update(orders::table.find(order_id))
                .set(orders::status.eq("accepted"))
                .execute(conn)
                .await?;

            diesel::insert_into(audit_events::table)
                .values(audit_events::order_id.eq(order_id))
                .execute(conn)
                .await?;

            Ok::<_, AutumnError>(())
        }
        .scope_boxed()
    })
    .await
}
```

The closure receives a raw `&mut PooledConnection`. Return `Ok(_)` to commit,
return `Err(_)` to roll back. Panics inside the closure also roll back without
committing partial work.

### Nesting policy

Nested `db.tx` calls are **rejected at runtime**:

```
Nested Db::tx calls are not supported
```

This is intentional. If you call a repository method inside `db.tx`, it
participates in the outer transaction — no new transaction is created, and the
same connection is reused. You will never accidentally start a nested
transaction by calling a repository method inside another one.

---

## Repository lifecycle hooks

Hooks give you a place to run model-scoped logic — validation, enrichment,
derived-field updates, side effects — without scattering that logic across
handlers.

### Opting in

```rust,no_run
use autumn_web::hooks::{MutationContext, MutationHooks, UpdateDraft};

#[derive(Clone, Default)]
pub struct ArticleHooks;

impl MutationHooks for ArticleHooks {
    type Model = Article;
    type NewModel = NewArticle;
    type UpdateModel = UpdateArticle;

    // override only the methods you need
}

#[repository(Article, hooks = ArticleHooks)]
pub trait ArticleRepository {}
```

The hooks struct must implement `Default` and `Clone`. Every method has a
default no-op implementation, so you only pay for what you override.

---

### `MutationContext`

Every hook receives a `&mut MutationContext`. It carries:

| Field | Type | Description |
|---|---|---|
| `op` | `MutationOp` | `Create`, `Update`, or `Delete` |
| `actor` | `Option<String>` | User ID or service name |
| `request_id` | `Option<String>` | UUID v4, auto-generated per mutation |
| `now` | `DateTime<Utc>` | Timestamp, auto-populated |
| `invalidate_keys` | `Vec<String>` | User-managed list of cache keys; Autumn collects them but does not act on them automatically |
| `idempotency_key` | `Option<String>` | Scoped HTTP idempotency key |

Useful methods:

```rust,no_run
// Push a key into ctx.invalidate_keys.
// Autumn does not consume this list automatically — read it yourself in an
// after_*_commit hook or cache middleware to perform the actual invalidation.
ctx.invalidate(format!("article:{}", record.id));

// Set an explicit idempotency key for durable side-effect deduplication
ctx.set_idempotency_key("my-scoped-key");
```

---

### `before_create`

Called before the `INSERT`, inside the transaction. Receives a mutable
reference to the new-record struct so you can enrich or normalize it before
it is written.

```rust,no_run
async fn before_create(
    &self,
    ctx: &mut MutationContext,
    new: &mut NewArticle,
) -> AutumnResult<()> {
    // Derive the slug from the title before the record is stored
    new.slug = slugify(&new.title);

    // Reject: returning Err prevents the INSERT and rolls back
    if new.title.trim().is_empty() {
        return Err(AutumnError::validation("title", "can't be blank"));
    }

    Ok(())
}
```

Returning `Err` prevents the mutation entirely. No SQL runs.

---

### `before_update`

Called before the `UPDATE`, inside the transaction. Receives an `UpdateDraft<T>`
that holds both the original (`before`) and proposed (`after`) model state.

```rust,no_run
async fn before_update(
    &self,
    ctx: &mut MutationContext,
    draft: &mut UpdateDraft<Article>,
) -> AutumnResult<()> {
    // Re-derive the slug only when the title actually changed
    if draft.after.title != draft.before.title {
        draft.after.slug = slugify(&draft.after.title);
    }

    // Stamp approved_at when status transitions to Approved
    if draft.after.status == Status::Approved && draft.before.status != Status::Approved {
        draft.after.approved_at = Some(ctx.now);
    }

    Ok(())
}
```

#### `UpdateDraft<T>`

| Method | Description |
|---|---|
| `draft.before()` | Reference to the original model |
| `draft.after()` | Reference to the proposed model |
| `draft.after_mut()` | Mutable reference to the proposed model |
| `draft.into_after()` | Consume the draft, return the proposed model |

#### `DraftField<T>` — per-field accessors

The `#[model]` macro generates per-field accessors that return a `DraftField`
borrowing from the draft. This lets you inspect and override individual fields
without juggling split borrows manually:

```rust,no_run
// Reading
draft.status().before()             // &Status  — value before mutation
draft.status().after()              // &Status  — proposed value
draft.status().changed()            // bool
draft.status().changed_to(&Status::Published)  // bool

// For Option<T> fields
draft.published_at().was_set()      // None → Some transition
draft.published_at().was_cleared()  // Some → None transition

// Overriding
draft.slug().set(slugify(&draft.after.title));
```

---

### `before_delete`

Called before the `DELETE`, inside the transaction. Receives a read-only
reference to the record about to be deleted.

```rust,no_run
async fn before_delete(
    &self,
    ctx: &mut MutationContext,
    record: &Article,
) -> AutumnResult<()> {
    if record.status == Status::Published {
        return Err(AutumnError::validation("status", "published articles cannot be deleted"));
    }
    Ok(())
}
```

---

### `after_create` and `after_update`

Called after the SQL runs, still inside the transaction boundary. Returning
`Err` rolls back the **entire mutation**, including the SQL that already ran.

These hooks do not receive the active database connection, so they cannot
issue additional SQL on the same transaction. Their main uses are:

- Business-level validation against the persisted state (reject with `Err` to rollback)
- In-memory bookkeeping (e.g., updating a cache struct held in app state)
- Collecting cache keys via `ctx.invalidate()` so a post-commit step can act on them

```rust,no_run
async fn after_create(
    &self,
    ctx: &mut MutationContext,
    record: &Article,
) -> AutumnResult<()> {
    // Collect cache keys to invalidate once the transaction commits.
    ctx.invalidate(format!("articles:author:{}", record.author_id));
    ctx.invalidate("articles:recent");
    Ok(())
}
```

If you need to write to a secondary table atomically with the mutation, do
that inside `db.tx` in the handler rather than in a hook.

---

### Hook error semantics — summary

| Hook | Error behavior |
|---|---|
| `before_create` | Prevents INSERT; transaction rolls back |
| `before_update` | Prevents UPDATE; transaction rolls back |
| `before_delete` | Prevents DELETE; transaction rolls back |
| `after_create` | INSERT has run but transaction rolls back |
| `after_update` | UPDATE has run but transaction rolls back |
| `after_create_commit` | Error is logged and counted; mutation is already committed |
| `after_update_commit` | Error is logged and counted; mutation is already committed |
| `after_delete_commit` | Error is logged and counted; mutation is already committed |

---

### `Patch<T>` — partial-update payloads

`Patch<T>` is a tri-state enum used in update changesets (the `UpdateModel`).
It distinguishes three cases that a plain `Option<T>` cannot:

```rust,no_run
pub enum Patch<T> {
    Unchanged,  // field was absent from the request — do nothing
    Set(T),     // field was explicitly set to a value
    Clear,      // field was explicitly set to null
}
```

When deserializing a JSON PATCH body, an absent field deserializes as
`Unchanged`, a `null` value deserializes as `Clear`, and any other value
deserializes as `Set(v)`. Mark fields with `#[serde(default)]` to wire
this up automatically.

```rust,no_run
// All three states are observable in before_update:
match draft.after.published_at {
    Patch::Unchanged => {} // caller didn't mention it
    Patch::Set(ts)   => {} // caller set it to a specific timestamp
    Patch::Clear     => {} // caller explicitly cleared it (→ NULL)
}
```

---

## `after_commit` — post-commit process-local callbacks

Some side effects must not fire against rolled-back data — job enqueues,
outbound emails, external API calls. `after_commit` callbacks are registered
inside a `db.tx` block and run only if the transaction commits successfully.
If the transaction rolls back, the callbacks are discarded.

### `register_after_commit`

```rust,no_run
use autumn_web::db::register_after_commit;

async fn publish_article(mut db: Db) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... UPDATE article status ...

        register_after_commit(|| async move {
            // Runs only if the UPDATE commits.
            // Failures here are logged but do not rollback the mutation.
            cdn::purge_cache("articles").await?;
            Ok(())
        })
        .await;

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

### Jobs — `enqueue_after_commit`

```rust,no_run
use autumn_web::job::enqueue_after_commit;

async fn create_user(mut db: Db) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... INSERT user ...

        // Enqueued only if the INSERT commits. No orphaned jobs.
        enqueue_after_commit("send_welcome_email", &args).await?;

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

For crash-safe job enqueue on Postgres, use `enqueue_in_tx` / `enqueue_on_conn`
instead — these write the job row inside the same transaction so the job and
the domain data commit atomically. See [Jobs → Transactional enqueue](jobs.md#transactional-enqueue).

### Mail — `deliver_later` (auto-deferred)

`Mailer::deliver_later` automatically detects a surrounding `db.tx` and defers
dispatch until after commit. No code change needed.

```rust,no_run
async fn register_user(mut db: Db, mailer: Mailer) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // ... INSERT user ...

        // Automatically deferred; no mail for rolled-back registrations.
        AccountMailer.deliver_later_welcome(&mailer, email, username);

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

To bypass deferral and dispatch immediately regardless of transaction state,
call `deliver_later_eager` instead.

### The crash-safety gap

`after_commit` callbacks are process-local. The sequence is:

```
Postgres confirms COMMIT → Tokio spawns callback → callback runs
```

If the process exits between the first and second step, the callback is lost.
For side effects that must survive process crashes, write a durable record
(outbox row, Postgres job row) inside the transaction itself, then have a
worker drain it. The callback can still be a useful wake-up hint, but the
durable record must be the source of truth.

### Observability

A process-level counter tracks `after_commit` failures (job broker down, SMTP
unreachable, etc.):

```rust,no_run
autumn_web::db::AFTER_COMMIT_FAILURES_TOTAL.load(std::sync::atomic::Ordering::Relaxed)
```

A non-zero value means at least one committed transaction's side effect was not
delivered. Scrape this counter in your metrics handler or health dashboard.

---

## `after_*_commit` hooks — durable, crash-safe

The `after_create_commit`, `after_update_commit`, and `after_delete_commit`
hook methods fire after the transaction has durably committed to Postgres, and
unlike the process-local `after_commit` callbacks above, they survive process
crashes.

### Opting in

Enable them per repository with `commit_hooks = true`:

```rust,no_run
#[repository(Article, hooks = ArticleHooks, commit_hooks = true)]
pub trait ArticleRepository {}
```

This tells the generated code to write an intent row into Autumn's
framework-owned `autumn_repository_commit_hooks` table in the same transaction
as the mutation. Workers claim and execute those rows using Postgres row locks,
so a process exit before execution is recovered by retry rather than silent
loss.

### Implementing the hooks

```rust,no_run
impl MutationHooks for ArticleHooks {
    // ...

    async fn after_create_commit(
        &self,
        ctx: &mut MutationContext,
        record: &Article,
    ) -> AutumnResult<()> {
        // Runs after the INSERT has durably committed.
        // Errors are logged and retried — they do NOT rollback the mutation.
        NotificationMailer
            .deliver_later_new_article(ctx.mailer(), record)
            .await
    }

    async fn after_update_commit(
        &self,
        ctx: &mut MutationContext,
        record: &Article,
    ) -> AutumnResult<()> {
        ctx.invalidate(format!("article:{}", record.id));
        cdn::purge_article(record.id).await
    }

    async fn after_delete_commit(
        &self,
        ctx: &mut MutationContext,
        record: &Article,
    ) -> AutumnResult<()> {
        search_index::delete(record.id).await
    }
}
```

### How the durable queue works

| State | Meaning |
|---|---|
| `enqueued` | Row written in same transaction as mutation; ready to claim |
| `running` | Claimed by a worker; heartbeat renewed every 15 s |
| `completed` | Successfully executed |
| `failed` | Exhausted retries (default: 5, with exponential backoff) |

If a worker crashes mid-execution, the stale claim (> 60 s without heartbeat)
is recovered by another worker.

Execution is **at-least-once**. A transient failure causes the hook to be
retried (up to 5 attempts, exponential backoff), and stale-claim recovery can
also re-execute a hook that appeared to start but never completed. Idempotency
keys deduplicate rows produced by a _retried HTTP request_ (the same logical
mutation submitted twice), but a single mutation's commit hook can still run
more than once due to retries. Design `after_*_commit` implementations to be
idempotent.

### Error semantics

Failures in `after_*_commit` are **not** propagated back to the caller and do
**not** rollback the committed mutation. They are logged and counted. If the
failure is persistent, the row reaches `failed` state and requires manual
recovery or investigation. Design these hooks to be idempotent.

---

## Bulk operations

All bulk methods participate in the same transaction model as single-record
mutations.

### Available methods

```rust,no_run
// Insert a batch of records
repo.save_many(&[NewArticle { ... }, NewArticle { ... }]).await?;

// Insert, skipping invalid rows rather than aborting the whole batch
let (saved, errors) = repo.save_many_skip_invalid(&rows).await?;

// Apply the same changeset to multiple records by ID
// UpdateModel fields are Patch<T>, not Option<T>
repo.update_many(&[1, 2, 3], &UpdateArticle {
    status: Patch::Set(Status::Archived),
    ..Default::default()
}).await?;

// Delete multiple records by ID
repo.delete_many(&[4, 5, 6]).await?;

// Insert-or-update by primary key — only available on hook-free repositories
repo.upsert_many(&[article_a, article_b]).await?;
```

### Transaction behavior

**Without hooks:** all bulk methods are direct SQL — a single batch statement,
no transaction overhead beyond what Postgres provides for a single statement.

**With hooks:** the entire batch — all before hooks, the batch SQL, and all
after hooks — is wrapped in a single transaction. If any hook or the SQL
returns an error, the entire batch rolls back.

```
save_many with hooks:
  BEGIN
    before_create(record_0)
    before_create(record_1)
    ...
    INSERT INTO ... VALUES (...), (...), ...   ← single batch query
    after_create(record_0)
    after_create(record_1)
    ...
    stage after_create_commit rows             ← if commit_hooks = true
  COMMIT
  → after_create_commit runs for each record
```

`update_many` and `delete_many` with hooks pay one extra round-trip: a
`SELECT ... FOR UPDATE` to fetch the current state of each record so that
`before_update` and `before_delete` can inspect it.

### Participating in `db.tx`

Bulk calls inside a `db.tx` block join the outer transaction — no nested
transaction is started.

```rust,no_run
db.tx(|conn| async move {
    // Participates in the outer transaction.
    // If either call fails, both roll back.
    repo.save_many(&new_articles).await?;
    repo.update_many(&stale_ids, &update).await?;
    Ok::<_, AutumnError>(())
}.scope_boxed())
.await?;
```

### `save_many_skip_invalid`

When bulk-importing dirty external data you may want to save the valid rows
and surface errors for the invalid ones rather than aborting the entire batch.

```rust,no_run
let rows: Vec<NewArticle> = parse_csv(upload)?;
let (saved, errors) = repo.save_many_skip_invalid(&rows).await?;

for (index, error) in &errors {
    tracing::warn!(row = index, error = %error, "skipped invalid row");
}
```

`before_create` hook failures are filtered out immediately. If the resulting
batch insert fails due to a database constraint (e.g. a unique violation),
Autumn falls back to row-by-row insertion for that chunk so that individual
constraint failures are isolated rather than aborting all remaining valid rows.

### `upsert_many` and hooks — a compile-time guard

`upsert_many` uses `INSERT ... ON CONFLICT (id) DO UPDATE`. Whether a
given row will insert or update is determined by Postgres at execution time —
not before the query runs. That makes it impossible to call the correct hook
(`before_create` vs `before_update`) before sending the statement.

To prevent silently bypassing hooks, calling `upsert_many` on a repository
that has hooks configured is **rejected at compile time**. Use `save_many` or
`update_many` explicitly when hooks are in play.

### Parameter ceiling and chunking

Postgres supports at most 65,535 bound parameters per statement. Autumn
calculates the maximum chunk size for your model's column count and splits
large batches automatically. You never need to chunk manually.

| Model columns | Max records per chunk |
|---|---|
| 5 | 1,000 (capped) |
| 10 | 1,000 (capped) |
| 50 | 1,000 (capped) |

Each chunk is inserted as a separate statement within the same transaction, so
atomicity is preserved across chunks.

---

## Decision guide

| What you need | Use |
|---|---|
| Multiple tables written atomically in a handler | `db.tx` |
| Validate or normalize a record before every insert | `before_create` |
| Derive a field from another on every update (e.g. slug from title) | `before_update` |
| Prevent deletion based on model state | `before_delete` |
| Write a secondary table atomically with the mutation | `db.tx` in the handler |
| Enqueue a job only if the DB write commits (no crash safety needed) | `enqueue_after_commit` inside `db.tx` |
| Crash-safe job enqueue on Postgres | `enqueue_in_tx` / `enqueue_on_conn` |
| Send email only if the write commits | `deliver_later` inside `db.tx` (auto-deferred) |
| Post-commit side effect scoped to a single repository model | `after_*_commit` with `commit_hooks = true` |
| Crash-safe post-commit side effect | `after_*_commit` with `commit_hooks = true` |
| Custom post-commit side effect in a handler | `register_after_commit` inside `db.tx` |

---

## Footguns

### Nested `db.tx` is rejected at runtime

There is no savepoint support. If code deeper in the call stack tries to open
a second `db.tx` while one is already active, it returns `Err` immediately:

```
Nested Db::tx calls are not supported
```

This is a runtime `Err`, not a panic — the outer transaction is still live and
will roll back normally when the error propagates. The fix is to pass the
connection down rather than calling `db.tx` again. If you call a repository
method inside `db.tx`, it already participates in the outer transaction — there
is no need to nest.

### `after_commit` callbacks are not crash-safe

Registering a callback with `register_after_commit` (or calling
`enqueue_after_commit` or `deliver_later`) does not make the side effect
durable. If the process exits in the window between Postgres confirming the
commit and Tokio executing the callback, the side effect is lost with no
record of it. Use `after_*_commit` hooks with `commit_hooks = true`, or write
a durable outbox row in the transaction, if you need guaranteed delivery.

### `after_*` hook errors roll back the mutation

`after_create` and `after_update` run inside the transaction boundary. If you
return `Err`, the entire mutation rolls back — including the `INSERT` or
`UPDATE` that already ran at the SQL level. This is the intended behaviour for
hard validation, but returning `Err` from a hook that performs a slow or
unreliable operation (e.g., an HTTP call, an in-memory cache write that can
fail) adds latency and potential rollbacks to every mutation.

For side effects that are not transactionally required (notifications, cache
invalidation), use `after_*_commit` hooks or `register_after_commit` instead,
so a failure there does not rollback the mutation.

### `before_update` pays a `SELECT FOR UPDATE` cost

Both single-record `.update()` and bulk `update_many()` perform a
`SELECT ... FOR UPDATE` before running `before_update` hooks, because the hook
needs the existing record state. This is an extra round-trip to the database.
If a repository has no update hooks, this query is not issued. Do not add
`before_update` hooks purely for documentation purposes.

### `update_many` applies one changeset to every record

```rust,no_run
// Sets status = "archived" on ALL three records
// UpdateModel fields are Patch<T> — use Patch::Set, not Some(...)
repo.update_many(&[1, 2, 3], &UpdateArticle {
    status: Patch::Set(Status::Archived),
    ..Default::default()
}).await?;
```

There is no per-record changeset variant. If you need different changes per
record, loop over `repo.update()` calls inside a `db.tx`.

### `upsert_many` silently bypasses hooks — and is blocked at compile time

There is no runtime fallback that would call the "right" hook for upsert. The
compile-time rejection is there to prevent a subtle bug where hooks appear to
be configured but are never called. If you see the compile error, you need to
decide: does this data need `save_many` (inserts with `before_create`) or
`update_many` (updates with `before_update`)?

---

## Worked example — all layers together

The scenario: a content platform where publishing an article must:

- normalize the slug and default status on create
- derive slug and stamp `published_at` only on the first draft→published transition
- collect cache keys to invalidate after the publish commits
- dispatch a durable webhook after commit
- enqueue a low-priority summary job only if the batch publish commits

```rust,no_run
// hooks.rs
use autumn_web::hooks::{MutationContext, MutationHooks, Patch, UpdateDraft};

#[derive(Clone, Default)]
pub struct ArticleHooks;

impl MutationHooks for ArticleHooks {
    type Model = Article;
    type NewModel = NewArticle;
    type UpdateModel = UpdateArticle;

    // Normalize on create: derive slug, set default status, reject blank title.
    async fn before_create(
        &self,
        _ctx: &mut MutationContext,
        new: &mut NewArticle,
    ) -> AutumnResult<()> {
        if new.title.trim().is_empty() {
            return Err(AutumnError::validation("title", "can't be blank"));
        }
        new.slug = slugify(&new.title);
        new.status = Status::Draft;
        Ok(())
    }

    // Derive slug when title changes; stamp published_at on the first
    // draft → published transition only (not on subsequent edits to an
    // already-published article).
    async fn before_update(
        &self,
        ctx: &mut MutationContext,
        draft: &mut UpdateDraft<Article>,
    ) -> AutumnResult<()> {
        if draft.after.title != draft.before.title {
            draft.after.slug = slugify(&draft.after.title);
        }
        // changed_to checks both that the value changed AND equals Published,
        // so re-saving an already-published article is a no-op here.
        if draft.status().changed_to(&Status::Published) {
            draft.after.published_at = Some(ctx.now);
        }
        Ok(())
    }

    // Mark cache keys to invalidate. after_update runs inside the transaction
    // boundary (returning Err rolls back), but does not receive the active
    // connection — use ctx.invalidate_keys in an after_*_commit hook to act
    // on them once the data is durably committed.
    async fn after_update(
        &self,
        ctx: &mut MutationContext,
        record: &Article,
    ) -> AutumnResult<()> {
        ctx.invalidate(format!("article:{}", record.id));
        ctx.invalidate(format!("articles:author:{}", record.author_id));
        Ok(())
    }

    // Durable post-commit webhook — at-least-once, so dispatch must be
    // idempotent. Survives process crashes via the framework queue table.
    async fn after_update_commit(
        &self,
        ctx: &mut MutationContext,
        record: &Article,
    ) -> AutumnResult<()> {
        // Only dispatch the webhook when the article was just published.
        // Because after_*_commit only has the final record (not the before
        // state), use published_at being newly set as the signal — if it was
        // already set before the update, the webhook has already fired.
        if record.status == Status::Published {
            webhooks::dispatch("article.published", record).await?;
        }
        // Act on cache keys collected in after_update.
        for key in &ctx.invalidate_keys {
            cache::invalidate(key).await?;
        }
        Ok(())
    }
}

// repositories.rs
#[repository(Article, hooks = ArticleHooks, commit_hooks = true)]
pub trait ArticleRepository {}

// handlers.rs — publish a batch of articles, also write the author's
// article_count atomically (requires a connection, so it lives in db.tx,
// not in a hook), and enqueue a summary job only if the tx commits.
async fn bulk_publish(mut db: Db, repo: ArticleRepo, ids: Vec<i64>) -> AutumnResult<()> {
    db.tx(|conn| async move {
        // update_many runs before_update + SQL + after_update inside this tx.
        // after_update_commit rows are staged here, executed after COMMIT.
        // UpdateModel fields are Patch<T> — use Patch::Set, not Some(...).
        let published = repo.update_many(&ids, &UpdateArticle {
            status: Patch::Set(Status::Published),
            ..Default::default()
        }).await?;

        // Increment the author's article count atomically inside the same tx.
        // This is done here (not in a hook) because it needs a DB connection.
        let author_ids: Vec<i64> = published.iter().map(|a| a.author_id).collect();
        diesel::update(authors::table.filter(authors::id.eq_any(&author_ids)))
            .set(authors::article_count.eq(authors::article_count + 1))
            .execute(conn)
            .await?;

        // Enqueued only if the entire tx commits. Not crash-safe on its own —
        // acceptable here because it's a low-priority summary, not a delivery
        // guarantee. For crash safety, use enqueue_in_tx instead.
        enqueue_after_commit("bulk_publish_summary", &BulkPublishArgs { ids }).await?;

        Ok::<_, AutumnError>(())
    }.scope_boxed())
    .await
}
```

Execution order when `bulk_publish` is called with `ids = [1, 2, 3]`:

```
BEGIN
  SELECT id, ... FROM articles WHERE id IN (1,2,3) FOR UPDATE
  before_update(article_1)  → derive slug if title changed, stamp published_at
  before_update(article_2)  → ...
  before_update(article_3)  → ...
  UPDATE articles SET status='published', published_at=... WHERE id IN (1,2,3)
  after_update(article_1)   → ctx.invalidate("article:1"), ctx.invalidate("articles:author:...")
  after_update(article_2)   → ...
  after_update(article_3)   → ...
  UPDATE authors SET article_count = article_count + 1 WHERE id IN (...)
  INSERT INTO autumn_repository_commit_hooks ... (3 rows)
COMMIT
↓
Tokio spawns: enqueue_after_commit("bulk_publish_summary", ...)
Workers claim: after_update_commit(article_1) → webhooks::dispatch(...), cache::invalidate(...)
               after_update_commit(article_2) → ...
               after_update_commit(article_3) → ...
```

If the database returns an error at any point before `COMMIT`, everything
above the line rolls back — no partial publishes, no phantom author-count
increments, no stale commit-hook rows. The `enqueue_after_commit` callback is
discarded. The `after_update_commit` rows were never committed, so workers
never see them.
