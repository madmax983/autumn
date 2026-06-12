# Migrations

Autumn embeds [Diesel](https://diesel.rs/) migrations in the compiled binary and
runs them at startup (dev) or via `autumn migrate run` (production). This guide
covers the advisory-lock serialisation that prevents schema divergence during
rolling deploys, how to monitor contention, and what to expect on non-Postgres
backends.

---

## Advisory lock serialisation

When several replicas boot at the same time or when `autumn migrate run` is
called from multiple deployment steps concurrently, every instance would
naively race to apply the same pending migrations. Diesel wraps each migration
in a transaction, so two processes applying the same migration can deadlock,
fail mid-DDL, or leave the schema half-applied.

Autumn prevents this by acquiring a **PostgreSQL session-level advisory lock**
before reading the pending-migration list. Only one process holds the lock at a
time; the rest wait (polling every 500 ms). Once the winner finishes, waiters
re-read the migration table, find no pending work, and exit successfully.

The lock covers:

* The embedded application migrations run by `AppBuilder::migrations(…)`.
* The Autumn framework migrations run by `autumn migrate run`.

---

## Lock key

The advisory lock uses a single `bigint` key:

```
MIGRATION_ADVISORY_LOCK_KEY = 0x6175_746E_5F6D_6967  (7 021 124 476 890 851 687)
```

The value is the big-endian encoding of the ASCII bytes `autn_mig`. It is
**stable across framework versions** so you can add permanent alerting rules
without consulting the source code.

PostgreSQL splits a `pg_advisory_lock(bigint)` key into two 32-bit halves
stored in `pg_locks`:

| Column     | Value        | Derivation            |
|------------|--------------|-----------------------|
| `classid`  | 1 635 087 470 | upper 32 bits of key |
| `objid`    | 1 601 005 927 | lower 32 bits of key |
| `objsubid` | 1            | session-level lock    |

---

## Monitoring contention

```sql
-- Active migration lock holders and waiters
SELECT
    pid,
    granted,
    mode,
    (SELECT query FROM pg_stat_activity WHERE pid = l.pid) AS query
FROM pg_locks l
WHERE locktype = 'advisory'
  AND classid = 1635087470
  AND objid   = 1601005927
  AND objsubid = 1;
```

A row with `granted = false` means a waiter is queued behind the current lock
holder. If rows remain indefinitely after a deploy, check whether a migration
process crashed mid-run; PostgreSQL will release the lock when the connection
closes.

---

## Wait timeout

The default wait is **60 seconds**. If the lock is not acquired within that
window the process fails with:

```
migration advisory lock not acquired within 60s;
another process may still be running migrations
```

The timeout can be overridden per call when using the Rust API:

```rust
use autumn_web::migrate::{run_pending_locked, DEFAULT_LOCK_WAIT_TIMEOUT};
use std::time::Duration;

// Use the default (60 s)
run_pending_locked(database_url, MIGRATIONS, None)?;

// Override to 120 s
run_pending_locked(database_url, MIGRATIONS, Some(Duration::from_secs(120)))?;
```

For the `autumn migrate run` CLI the timeout is always the default.

---

## Wrapping an external migration process

If you invoke an external migration tool (e.g. a raw `diesel` subprocess) and
want it covered by the same advisory lock, use `hold_migration_lock`:

```rust
use autumn_web::migrate::{hold_migration_lock, DEFAULT_LOCK_WAIT_TIMEOUT};

let _guard = hold_migration_lock(database_url, DEFAULT_LOCK_WAIT_TIMEOUT)?;
// Lock is held for the lifetime of `_guard`.
// Run external process here …
// Lock is released when `_guard` drops.
```

This is exactly what `autumn migrate run` does internally before shelling out
to the `diesel` CLI.

---

## Non-Postgres backends

Advisory locks are a **PostgreSQL-specific** primitive. SQLite and in-memory
test harnesses do not support them.

* **SQLite / in-memory** — These backends are single-process by nature and do
  not need cross-process serialisation. Call `run_pending` directly; no lock is
  acquired or needed.
* **Tests using `TestDb`** — `TestDb` starts a real Postgres container, so the
  advisory lock is acquired normally.

If you write tests that call `run_pending_locked` against a non-Postgres
database the connection will fail before the lock query is issued, and the
function returns `MigrationError::Connection`.

---

## Log output

| Level   | Event                                           |
|---------|-------------------------------------------------|
| `INFO`  | Lock key and timeout when acquisition starts   |
| `INFO`  | Lock acquired                                   |
| `DEBUG` | Waiting message (emitted every ~500 ms)         |
| `INFO`  | Lock released                                   |
| `ERROR` | Lock timeout or migration failure               |

Set `RUST_LOG=autumn_web::migrate=debug` to see the full waiting timeline.
