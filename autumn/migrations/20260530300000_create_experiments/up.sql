-- A/B experiments: deterministic bucketing, sticky assignments, exposure telemetry.
--
-- autumn_experiments holds the current configuration of each experiment.
-- autumn_experiment_assignments records sticky actor → variant assignments.
-- autumn_experiment_overrides pins QA/staff actors to specific variants.
-- autumn_experiment_changes is the append-only audit log.
--
-- Postgres LISTEN/NOTIFY: any weight/state mutation sends
-- `NOTIFY autumn_experiments, <name>` so all replicas can reload cached
-- experiment configs within seconds (consistent with feature-flags and
-- runtime-config substrate).

CREATE TYPE autumn_experiment_state AS ENUM ('draft', 'running', 'concluded', 'archived');

CREATE TABLE IF NOT EXISTS autumn_experiments (
    id               BIGSERIAL                NOT NULL PRIMARY KEY,
    name             TEXT                     NOT NULL UNIQUE,
    description      TEXT,
    state            autumn_experiment_state  NOT NULL DEFAULT 'draft',
    variants         JSONB                    NOT NULL DEFAULT '[]',
    winner           TEXT,
    exclusion_group  TEXT,
    created_at       TIMESTAMPTZ              NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ              NOT NULL DEFAULT NOW()
);

COMMENT ON COLUMN autumn_experiments.variants IS
    'JSON array of {"name": string, "weight": integer} objects. '
    'Weights are relative; they do not need to sum to 100.';
COMMENT ON COLUMN autumn_experiments.winner IS
    'Set on conclusion: the winning variant name. NULL while running.';
COMMENT ON COLUMN autumn_experiments.exclusion_group IS
    'Actors in any experiment with the same group are excluded from siblings.';

CREATE INDEX IF NOT EXISTS idx_autumn_experiments_name
    ON autumn_experiments (name);
CREATE INDEX IF NOT EXISTS idx_autumn_experiments_state
    ON autumn_experiments (state);

-- Sticky assignments: once an actor is assigned to a variant the row is
-- recorded here and returned on all subsequent assign() calls without
-- re-computing the bucket.  is_override=true when the assignment came from
-- an operator override rather than weight-based bucketing.
CREATE TABLE IF NOT EXISTS autumn_experiment_assignments (
    id               BIGSERIAL   NOT NULL PRIMARY KEY,
    experiment       TEXT        NOT NULL,
    actor            TEXT        NOT NULL,
    variant          TEXT        NOT NULL,
    is_override      BOOLEAN     NOT NULL DEFAULT FALSE,
    assigned_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (experiment, actor)
);

CREATE INDEX IF NOT EXISTS idx_autumn_exp_assignments_experiment
    ON autumn_experiment_assignments (experiment);
CREATE INDEX IF NOT EXISTS idx_autumn_exp_assignments_actor
    ON autumn_experiment_assignments (actor);

-- QA/staff overrides: pins an actor to a specific variant bypassing weights.
-- Takes precedence over sticky assignments on each assign() call.
CREATE TABLE IF NOT EXISTS autumn_experiment_overrides (
    id          BIGSERIAL   NOT NULL PRIMARY KEY,
    experiment  TEXT        NOT NULL,
    actor       TEXT        NOT NULL,
    variant     TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (experiment, actor)
);

CREATE INDEX IF NOT EXISTS idx_autumn_exp_overrides_experiment
    ON autumn_experiment_overrides (experiment, actor);

-- Audit log: every mutation (create, set_weights, state change, override) is
-- appended here with the operator identity and a timestamp.
CREATE TABLE IF NOT EXISTS autumn_experiment_changes (
    id          BIGSERIAL   NOT NULL PRIMARY KEY,
    experiment  TEXT        NOT NULL,
    mutation    TEXT        NOT NULL,
    actor       TEXT,
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

COMMENT ON COLUMN autumn_experiment_changes.mutation IS
    'Human-readable description: "created", "state=running", "set_weights", '
    '"concluded=treatment", "override=user:42:treatment", etc.';

CREATE INDEX IF NOT EXISTS idx_autumn_exp_changes_experiment_time
    ON autumn_experiment_changes (experiment, changed_at DESC);

-- LISTEN/NOTIFY trigger: replicas subscribe to autumn_experiments channel and
-- reload cached experiment configs whenever a change is recorded.
CREATE OR REPLACE FUNCTION autumn_notify_experiment_change()
RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('autumn_experiments', NEW.experiment);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER autumn_experiment_change_notify
AFTER INSERT ON autumn_experiment_changes
FOR EACH ROW EXECUTE FUNCTION autumn_notify_experiment_change();
