-- Feature flags: live-toggleable named gates with rollout and allowlist support.
--
-- autumn_feature_flags holds the current state of each flag.
-- feature_flag_changes is the append-only audit log — every enable/disable/
-- rollout/allowlist mutation records actor, mutation description, and timestamp.
--
-- Postgres LISTEN/NOTIFY: any write sends `NOTIFY autumn_flags, <key>` so all
-- running replicas can invalidate their in-process cache within seconds.

CREATE TABLE IF NOT EXISTS autumn_feature_flags (
    id               BIGSERIAL   PRIMARY KEY,
    key              TEXT        NOT NULL UNIQUE,
    description      TEXT,
    enabled          BOOLEAN     NOT NULL DEFAULT FALSE,
    rollout_pct      SMALLINT    NOT NULL DEFAULT 0 CHECK (rollout_pct BETWEEN 0 AND 100),
    actor_allowlist  TEXT        NOT NULL DEFAULT '[]',
    group_allowlist  TEXT        NOT NULL DEFAULT '[]',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

COMMENT ON COLUMN autumn_feature_flags.actor_allowlist IS
    'JSON array of actor_id strings always enabled regardless of other gates';
COMMENT ON COLUMN autumn_feature_flags.group_allowlist IS
    'JSON array of group name strings; membership resolved by the app group resolver';

CREATE INDEX IF NOT EXISTS idx_autumn_feature_flags_key
    ON autumn_feature_flags (key);
CREATE TABLE IF NOT EXISTS feature_flag_changes (
    id          BIGSERIAL   PRIMARY KEY,
    key         TEXT        NOT NULL,
    mutation    TEXT        NOT NULL,
    actor       TEXT,
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

COMMENT ON COLUMN feature_flag_changes.mutation IS
    'Human-readable description: "enabled", "disabled", "rollout=25", "allowed_actor=user:42", etc.';

CREATE INDEX IF NOT EXISTS idx_feature_flag_changes_key_time
    ON feature_flag_changes (key, changed_at DESC);

-- Notify channel: replicas subscribe to this channel so they can invalidate
-- their flag cache on any mutation without polling.
CREATE OR REPLACE FUNCTION autumn_notify_flag_change()
RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('autumn_flags', NEW.key);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER autumn_flag_change_notify
AFTER INSERT ON feature_flag_changes
FOR EACH ROW EXECUTE FUNCTION autumn_notify_flag_change();
