CREATE TABLE IF NOT EXISTS autumn_repository_commit_hooks (
    id                  TEXT        PRIMARY KEY,
    handler_key         TEXT        NOT NULL,
    hook_name           TEXT        NOT NULL,
    context             JSONB       NOT NULL DEFAULT '{}',
    record              JSONB       NOT NULL DEFAULT '{}',
    status              TEXT        NOT NULL DEFAULT 'enqueued',
    attempt             INTEGER     NOT NULL DEFAULT 1,
    max_attempts        INTEGER     NOT NULL DEFAULT 5,
    initial_backoff_ms  BIGINT      NOT NULL DEFAULT 1000,
    enqueued_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at          TIMESTAMPTZ,
    finished_at         TIMESTAMPTZ,
    claimed_by          TEXT,
    claimed_at          TIMESTAMPTZ,
    last_error          TEXT,
    run_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Workers claim ready rows with SELECT ... FOR UPDATE SKIP LOCKED.
CREATE INDEX IF NOT EXISTS idx_autumn_repository_commit_hooks_ready
    ON autumn_repository_commit_hooks (run_at ASC, enqueued_at ASC)
    WHERE status = 'enqueued';

-- Dispatchers only claim hooks whose generated runner is registered locally.
CREATE INDEX IF NOT EXISTS idx_autumn_repository_commit_hooks_handler_ready
    ON autumn_repository_commit_hooks (handler_key, run_at ASC)
    WHERE status = 'enqueued';

-- Stale-claim recovery lets another replica retry work abandoned by a dead worker.
CREATE INDEX IF NOT EXISTS idx_autumn_repository_commit_hooks_stale_recovery
    ON autumn_repository_commit_hooks (claimed_at)
    WHERE status = 'running';
