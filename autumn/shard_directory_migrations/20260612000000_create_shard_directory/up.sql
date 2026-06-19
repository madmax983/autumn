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

-- Cache-invalidation log + NOTIFY.
--
-- Every change to the directory records the affected tenant_key here and fires
-- `NOTIFY autumn_shard_directory, <tenant_key>`. The framework's
-- DirectoryShardRouter invalidation listener reads this append-only log on a
-- timestamp cursor and evicts that tenant's cached pin within seconds, so a
-- re-pin during a slot move takes effect well before the cache TTL would
-- expire. Logging via a TRIGGER (not app code) captures operator SQL writes to
-- the directory too, not just writes made through the framework. This is a
-- low-volume log (one row per tenant relocation).
CREATE TABLE IF NOT EXISTS _autumn_shard_directory_changes (
    id          BIGSERIAL   PRIMARY KEY,
    tenant_key  TEXT        NOT NULL,
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_autumn_shard_directory_changes_time
    ON _autumn_shard_directory_changes (changed_at);

CREATE OR REPLACE FUNCTION autumn_notify_shard_directory_change()
RETURNS trigger AS $$
BEGIN
    IF (TG_OP = 'DELETE') THEN
        INSERT INTO _autumn_shard_directory_changes (tenant_key) VALUES (OLD.tenant_key);
        PERFORM pg_notify('autumn_shard_directory', OLD.tenant_key);
        RETURN NULL;
    END IF;
    -- INSERT or UPDATE: invalidate the (new) key.
    INSERT INTO _autumn_shard_directory_changes (tenant_key) VALUES (NEW.tenant_key);
    PERFORM pg_notify('autumn_shard_directory', NEW.tenant_key);
    -- A primary-key rename also strands the old key's cache entry, so invalidate
    -- both ends of the rename.
    IF (TG_OP = 'UPDATE' AND OLD.tenant_key IS DISTINCT FROM NEW.tenant_key) THEN
        INSERT INTO _autumn_shard_directory_changes (tenant_key) VALUES (OLD.tenant_key);
        PERFORM pg_notify('autumn_shard_directory', OLD.tenant_key);
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS autumn_shard_directory_notify ON _autumn_shard_directory;
CREATE TRIGGER autumn_shard_directory_notify
AFTER INSERT OR UPDATE OR DELETE ON _autumn_shard_directory
FOR EACH ROW EXECUTE FUNCTION autumn_notify_shard_directory_change();
