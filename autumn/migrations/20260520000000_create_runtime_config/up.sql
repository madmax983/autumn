-- Runtime configuration store: live-tunable typed values.
--
-- runtime_config_values holds the currently active overrides for each key.
-- Only keys with an operator-set value appear here; when a key is absent
-- the application falls back to its compile-time schema default.
--
-- runtime_config_changes is the append-only audit log.  Every set/unset
-- operation records the actor, old value, new value, and a UTC timestamp.
-- Rows are never deleted; the table is surfaced in the admin UI and
-- emitted as a tracing event.

CREATE TABLE IF NOT EXISTS autumn_runtime_config_values (
    key         TEXT        NOT NULL PRIMARY KEY,
    raw_value   TEXT        NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_autumn_runtime_config_values_updated
    ON autumn_runtime_config_values (updated_at DESC);

CREATE TABLE IF NOT EXISTS autumn_runtime_config_changes (
    id          BIGSERIAL   PRIMARY KEY,
    key         TEXT        NOT NULL,
    old_value   TEXT,
    new_value   TEXT,
    actor       TEXT,
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Fast look-ups for `autumn config history <key>`
CREATE INDEX IF NOT EXISTS idx_autumn_runtime_config_changes_key_time
    ON autumn_runtime_config_changes (key, changed_at DESC);
