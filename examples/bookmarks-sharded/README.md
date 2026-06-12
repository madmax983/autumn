# Bookmarks Sharded

Framework-native horizontal sharding: two Postgres shards, one control
database, two stateless web replicas behind nginx — and **zero sharding
code in the application**. Routing is declared in `autumn.toml` and the
handlers use the `ShardedDb` / `Shards` extractors from the prelude.

## How routing works

```
tenant id ──hash──▶ logical slot (0..64) ──config map──▶ physical shard
```

The key→slot hash is deterministic across processes and Autumn versions
(both web replicas always agree). The slot→shard map is configuration:

```toml
[database]
primary_url = "postgres://.../bookmarks_control"   # control role: NOT sharded
slot_count  = 64                                   # choose once, before data lands

[[database.shards]]
name = "shard0"
primary_url = "postgres://.../bookmarks_shard0"
slots = ["0-31"]

[[database.shards]]
name = "shard1"
primary_url = "postgres://.../bookmarks_shard1"
slots = ["32-63"]
```

Resharding means moving whole slots: copy a slot's rows to the new
shard, flip its `slots` entry, deploy. Keys are never rehashed.

## Prerequisites

- Docker and Docker Compose (the stack runs three Postgres containers,
  two web replicas, nginx, and a one-shot migrator — nothing is needed
  on the host besides Docker)

## Quick Start

```bash
export AUTUMN_SECURITY__SIGNING_SECRET="$(openssl rand -hex 32)"
docker compose -f examples/bookmarks-sharded/docker-compose.yml up -d --build
```

The `bookmarks-migrate` one-shot job applies migrations to the control
database and then every shard before the web replicas start.

## Walkthrough

Create bookmarks for two tenants — each tenant's rows land on the shard
that owns its slot, and the response says which:

```bash
curl -s -X POST http://localhost:3000/api/bookmarks \
  -H 'Content-Type: application/json' -H 'X-Tenant-Id: acme' \
  -d '{"url":"https://rust-lang.org","title":"Rust","tag":"lang"}'

curl -s -H 'X-Tenant-Id: acme' http://localhost:3000/api/bookmarks
# {"shard":"shard0","bookmarks":[...]}   ← always the same shard for acme

curl -s -H 'X-Tenant-Id: globex' http://localhost:3000/api/bookmarks
# {"shard":"shard1","bookmarks":[]}      ← a different tenant, possibly a different shard
```

Repeat the list call: nginx alternates between the two web replicas, but
the shard never changes — the hash is process-independent. Verify on the
database side:

```bash
docker compose -f examples/bookmarks-sharded/docker-compose.yml \
  exec postgres-shard-0 psql -U autumn -d bookmarks_shard0 \
  -c 'SELECT tenant_id, count(*) FROM bookmarks GROUP BY 1;'
```

Cross-shard fan-out (concurrent, partial-failure friendly):

```bash
curl -s http://localhost:3000/api/stats
# {"shard0":{"slots":32,"bookmarks":3},"shard1":{"slots":32,"bookmarks":1}}
```

Per-shard health components and pool metrics:

```bash
curl -s http://localhost:3000/health | jq '.components | keys'
# [..., "db:shard:shard0", "db:shard:shard1"]
curl -s http://localhost:3000/actuator/metrics | jq '.database_shards'
```

Stop a shard and watch readiness degrade (and recover):

```bash
docker compose -f examples/bookmarks-sharded/docker-compose.yml stop postgres-shard-1
curl -s http://localhost:3000/api/stats     # shard1 reports an error, shard0 still answers
docker compose -f examples/bookmarks-sharded/docker-compose.yml start postgres-shard-1
```

## What stays on the control database

Framework state is **never sharded**: the `autumn_jobs` queue, Postgres
scheduler advisory locks, sessions, and feature flags all live on the
control role (`database.primary_url`). Only tenant data routes across
shards. There are **no cross-shard transactions** — a write spans exactly
one shard, and `/api/stats` aggregates independent per-shard snapshots.

## Things to try

- `autumn migrate status` prints a per-target table (control + each
  shard); `autumn migrate --shard shard1` targets one shard.
- Add `replica_url` to a shard: each shard is a full primary/replica
  topology, so the replica story composes with sharding.
- Implement a custom `ShardRouter` (`.with_shard_router(...)`) to pin a
  hot "whale" tenant to a dedicated shard — hash routing balances key
  count, not load.
