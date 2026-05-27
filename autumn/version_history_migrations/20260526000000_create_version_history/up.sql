-- Automatic record version history table (issue #700).
--
-- Every opted-in repository write (insert, update, delete) appends one
-- immutable row here. The table is append-only: there is no public API
-- to UPDATE or DELETE rows. Test teardown uses a separate escape hatch.
--
-- Schema notes:
--   table_name  TEXT  -- the Diesel table name (e.g. "posts")
--   tenant_id   TEXT  -- tenant scope for tenant_scoped repositories (nullable)
--   record_id   BIGINT -- the row PK; assumes BIGSERIAL / i64 PKs
--   op          TEXT  -- 'insert' | 'update' | 'delete'
--   actor       TEXT  -- authenticated user_id, or 'system'
--   request_id  TEXT  -- trace / correlation ID (nullable)
--   changes     JSONB -- array of { column, before, after, sensitive }
--   recorded_at TIMESTAMPTZ -- server UTC timestamp (NOT NULL, defaults to NOW())

CREATE TABLE IF NOT EXISTS _autumn_version_history (
    id          BIGSERIAL   PRIMARY KEY,
    table_name  TEXT        NOT NULL,
    tenant_id   TEXT,
    record_id   BIGINT      NOT NULL,
    op          TEXT        NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
    actor       TEXT        NOT NULL DEFAULT 'system',
    request_id  TEXT,
    changes     JSONB       NOT NULL DEFAULT '[]',
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE _autumn_version_history
    ADD COLUMN IF NOT EXISTS tenant_id TEXT;

CREATE INDEX IF NOT EXISTS idx_autumn_version_history_record
    ON _autumn_version_history (table_name, record_id, recorded_at ASC);

CREATE INDEX IF NOT EXISTS idx_autumn_version_history_tenant_record
    ON _autumn_version_history (table_name, tenant_id, record_id, recorded_at ASC);

CREATE INDEX IF NOT EXISTS idx_autumn_version_history_time
    ON _autumn_version_history (table_name, recorded_at ASC);

CREATE INDEX IF NOT EXISTS idx_autumn_version_history_actor
    ON _autumn_version_history (actor, recorded_at DESC);
