-- Explicit tenant → shard directory for DirectoryShardRouter (issue #1209 §4).
--
-- A control-plane lookup table that pins specific tenants to specific shards,
-- overriding the default slot-hash routing. Used for:
--   * "whale" tenants moved to a dedicated shard,
--   * tenants migrated between shards during a slot move,
--   * any case where ownership must not be a pure function of the hash.
--
-- This table lives on the CONTROL database only. `DirectoryShardRouter`
-- consults it (with a TTL cache) on each route; a missing row falls back to
-- the hash router, so partial population is fine — only relocated tenants
-- need a row.
--
-- Schema notes:
--   tenant_key  TEXT  -- the routing key (tenant id) this entry pins
--   shard_name  TEXT  -- the target shard's configured `[[database.shards]]` name
--   updated_at  TIMESTAMPTZ -- last write, for auditing / cache reasoning

CREATE TABLE IF NOT EXISTS _autumn_shard_directory (
    tenant_key  TEXT        NOT NULL PRIMARY KEY,
    shard_name  TEXT        NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Operators inspect "which tenants live on shard X" when planning slot moves.
CREATE INDEX IF NOT EXISTS idx_autumn_shard_directory_shard
    ON _autumn_shard_directory (shard_name);
