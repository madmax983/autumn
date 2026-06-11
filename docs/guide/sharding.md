# Horizontal Sharding

A single Postgres primary — even with read replicas — eventually becomes
the write bottleneck. Autumn's sharding story routes tenant data across
multiple independent Postgres databases while keeping the application
code shard-unaware: routing is declared in `autumn.toml`, and handlers
use the `ShardedDb` / `Shards` extractors from the prelude.

Sharding composes with the primary/replica story: **each shard is a full
primary + optional replica topology** of its own, with the same
`replica_fallback` semantics you already use on the control role.

## Keys, slots, and shards

```
routing key (tenant id) ──hash──▶ logical slot (0..slot_count) ──config map──▶ shard
```

Keys never map to physical shards directly. They hash onto a fixed set
of **logical slots** (`database.slot_count`, default 64), and each slot
is owned by exactly one shard per the configuration. Two properties fall
out of this split:

- **The key→slot hash is a permanent contract.** It is deterministic
  across processes, replicas, and Autumn versions (FNV-1a/splitmix64 —
  never the process-randomized std hasher), so every web replica routes
  the same tenant to the same shard, forever. Golden-vector tests in the
  framework pin it.
- **The slot→shard map is just configuration.** Resharding means moving
  whole slots: copy a slot's rows to the new shard, flip its `slots`
  entry, deploy. Keys are never rehashed.

`slot_count` is **choose-once**: changing it later re-routes every key —
a full reshard. The default 64 supports up to 64 physical shards.

## Configuration

```toml
[database]
# The CONTROL role. Framework state lives here and is never sharded.
primary_url = "postgres://db-control/app"
slot_count = 64

[[database.shards]]
name = "shard0"                              # stable identity for logs/metrics/health/CLI
primary_url = "postgres://db-shard0/app"
slots = ["0-31"]                             # indices and/or "A-B" inclusive ranges

[[database.shards]]
name = "shard1"
primary_url = "postgres://db-shard1/app"
slots = ["32-63"]
replica_url = "postgres://db-shard1-ro/app"  # optional: full topology per shard
replica_fallback = "primary"                 # falls back to [database] defaults
primary_pool_size = 4

[tenancy]
enabled = true
source = "header"        # the tenant id doubles as the shard routing key
```

`slots` is all-or-none: either every shard declares it (the map must
cover `0..slot_count` exactly once; an explicit empty list marks a
drained shard being decommissioned) or none does, in which case Autumn
auto-splits the slot space into contiguous even ranges **by declaration
order**. The auto-split is convenient to start with, but reordering
entries then moves data — pin explicit `slots` before any topology
change.

Environment overrides address shards positionally:

| Variable | Field |
|----------|-------|
| `AUTUMN_DATABASE__SLOT_COUNT` | `database.slot_count` |
| `AUTUMN_DATABASE__SHARDS__{i}__NAME` | `database.shards[i].name` |
| `AUTUMN_DATABASE__SHARDS__{i}__PRIMARY_URL` | `database.shards[i].primary_url` |
| `AUTUMN_DATABASE__SHARDS__{i}__SLOTS` | CSV, e.g. `"0-15,40,62-63"` |
| `AUTUMN_DATABASE__SHARDS__{i}__REPLICA_URL` | `database.shards[i].replica_url` |
| `AUTUMN_DATABASE__SHARDS__{i}__PRIMARY_POOL_SIZE` | `database.shards[i].primary_pool_size` |
| `AUTUMN_DATABASE__SHARDS__{i}__REPLICA_POOL_SIZE` | `database.shards[i].replica_pool_size` |
| `AUTUMN_DATABASE__SHARDS__{i}__REPLICA_FALLBACK` | `database.shards[i].replica_fallback` |

Pool sizes multiply across shards (N shards × pool_size + control +
replicas). Startup logs the aggregate as `total_max_connections` —
check it against your Postgres `max_connections` budget.

## Handlers

`ShardedDb` resolves the routing key automatically (a `ShardKeyOverride`
request extension, then the tenancy task-local, then direct tenant
extraction per `[tenancy]`), routes it, and checks out a connection to
the owning shard's primary. It dereferences to `AsyncPgConnection`
exactly like `Db`, and `tx()` has the same semantics — on that one
shard:

```rust
use autumn_web::prelude::*;

#[get("/bookmarks")]
async fn list(tenant: Tenant, mut db: ShardedDb) -> AutumnResult<Json<Vec<Bookmark>>> {
    // Several tenants share a shard: still filter by tenant.
    let rows = bookmarks::table
        .filter(bookmarks::tenant_id.eq(&tenant.0))
        .load::<Bookmark>(&mut *db)
        .await?;
    Ok(Json(rows))
}
```

`Shards` is the explicit API — extract once, route per call:

```rust
#[get("/users/{user_id}/report")]
async fn report(shards: Shards, Path(user_id): Path<i64>) -> AutumnResult<String> {
    let mut db = shards.db_for(user_id).await?;      // primary of the owning shard
    let mut ro = shards.read_for(user_id).await?;    // replica-aware read connection
    let mut admin = shards.db_on("shard0").await?;   // by name (admin paths)
    // ...
    Ok("ok".into())
}
```

Cross-shard fan-out runs concurrently (bounded) and collects per-shard
results in declaration order instead of short-circuiting, so one down
shard degrades an aggregate endpoint to a partial answer:

```rust
let counts = shards
    .each_shard(|shard, mut db| {
        let name = shard.name().to_owned();
        async move {
            let count = bookmarks::table.count().get_result::<i64>(&mut *db).await?;
            Ok((name, count))
        }
    })
    .await; // Vec<(ShardId, Result<(String, i64)>)>
```

Repositories generated by `#[repository]` extract over the **control**
pool. To run one against a shard, use the generated `with_pool`
constructor:

```rust
let shard = shards.set().route(&tenant_id).await?;
let repo = PgBookmarkRepository::with_pool(shard.primary_pool().clone());
```

(A `#[repository(tenant_scoped, sharded)]` mode that routes
automatically is planned; `across_tenants()` is inherently cross-shard,
which is why phase one keeps repository routing explicit.)

## There are no cross-shard transactions

Full stop. A transaction spans exactly one shard. A job that writes to
two tenants' shards can fail halfway; `each_shard` reads can observe
torn aggregates while writers are active. This is the design, not a gap
to fill later — giving up multi-object atomicity across partition
boundaries is what buys independent failure domains and linear write
scaling. Design workflows so each unit of work touches one shard, and
treat cross-shard aggregates as approximate.

## The control database

Framework state is **never sharded**. The `autumn_jobs` queue, Postgres
scheduler advisory locks, sessions, feature flags, and idempotency keys
all live on the control topology (`database.primary_url`/`url`), and the
plain `Db` extractor still points there. Startup fails fast if you
configure shards plus a Postgres-backed jobs/scheduler backend without a
control role.

Jobs that operate on sharded data should carry the shard key in their
payload and resolve the shard from state:

```rust
#[job]
async fn reindex_tenant(state: State<AppState>, payload: Json<Reindex>) -> AutumnResult<()> {
    let shard = state.shards().expect("sharded app").route(&payload.tenant_id).await?;
    let pool = shard.primary_pool().clone();
    // ...
    Ok(())
}
```

## Migrations

Startup auto-migrate (dev) and `autumn migrate` (prod) apply your
migrations to the control database first, then every shard in
declaration order, **failing fast** with a `target=` label on every log
line — a half-migrated fleet that boots is worse than a crashed deploy,
and already-migrated targets are skipped idempotently on retry.

```bash
autumn migrate                  # control + every shard, per-target summary
autumn migrate --shard shard1   # one shard
autumn migrate --control-only   # pre-shards behavior
autumn migrate status           # per-target pending/applied table
```

## Health, readiness, and metrics

Each shard registers a `db:shard:<name>` component in `/ready` and
`/actuator/health`. On every readiness probe it live-checks replica
connectivity and re-runs the migration parity comparison, gating that
shard's replica reads exactly like the control replica:

- replica unready + `replica_fallback = "fail_readiness"` → component
  `Down`, `/ready` fails, `read_for()` errors;
- replica unready + `replica_fallback = "primary"` → component stays
  `Up`, reads degrade to the shard's primary, replica state in details.

`/actuator/metrics` exposes a `database_shards` block with per-shard
primary/replica pool stats and slot counts, and shard-routed checkouts
tag their route metrics with `shard=<name>` so per-shard latency
separates.

## Custom routing and whale tenants

Hash routing balances **key count, not load** — and at tenant
granularity, a single hot "whale" tenant cannot be split. This is the
most common way multi-tenant sharding breaks in practice. Two escape
hatches:

1. Give the whale's slot range its own physical shard (slots are the
   unit of placement).
2. Install a directory router that pins specific tenants and falls back
   to the hash for everyone else:

```rust
use autumn_web::sharding::{ShardId, ShardKey, ShardRouter, ShardSet};

struct PinnedRouter;

impl ShardRouter for PinnedRouter {
    fn route<'a>(
        &'a self,
        key: ShardKey<'a>,
        shards: &'a ShardSet,
    ) -> futures::future::BoxFuture<'a, Result<ShardId, autumn_web::AutumnError>> {
        Box::pin(async move {
            if let ShardKey::Str("whale-corp") = key {
                // Dedicated shard for the hot tenant.
                return Ok(shards.by_name("whale").expect("configured").id());
            }
            let slot = shards.slot_for_key(key);
            Ok(shards.shard_for_slot(slot).expect("slot map covers all slots").id())
        })
    }
}

// autumn_web::app().with_shard_router(PinnedRouter)
```

Routing is async, so a directory router may consult a cache or the
control database. Custom pool construction per shard goes through
`DatabasePoolProvider::create_shard_topology`.

## Read-your-own-writes, × N

Each shard inherits the replica story's read-your-own-writes gap:
`read_for(key)` immediately after `db_for(key)` is the textbook
stale-read sequence, now on every shard independently. Use the shard's
primary (`db_for`) for read-after-write paths, exactly as you would with
the control replica.

## Resharding runbook

1. Pin explicit `slots` on every shard (if you started with the
   auto-split) and deploy — this changes nothing, it just makes the map
   explicit.
2. Stand up the new shard's database and add its `[[database.shards]]`
   entry with `slots = []` (a valid drained shard). Deploy; run
   `autumn migrate --shard <new>`.
3. For each slot to move: copy that slot's rows (`WHERE` on your tenant
   key filtered through `ShardSet::slot_for_key`, or dual-write at the
   application layer), then move the slot index from the old shard's
   `slots` to the new shard's and deploy. Writes for that slot now land
   on the new shard.
4. Delete the moved rows from the old shard once traffic confirms.

Moving a slot moves only that slot's keys — the framework's tests pin
this property (`moving_a_slot_in_config_moves_only_that_slot`).

## Testing

Deadpool builds pools lazily, so sharded states are constructible in
tests without N running databases. The transactional test harness wraps
only the control pool today: sharded checkouts in tests run against
real (non-rolled-back) connections.

See [`examples/bookmarks-sharded`](../../examples/bookmarks-sharded)
for a runnable Docker Compose stack: two shards, a control database,
two web replicas behind nginx, and a one-shot multi-target migrator.
