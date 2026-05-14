CREATE TABLE IF NOT EXISTS autumn_jobs (
    id                TEXT        PRIMARY KEY,
    name              TEXT        NOT NULL,
    payload           JSONB       NOT NULL DEFAULT '{}',
    status            TEXT        NOT NULL DEFAULT 'enqueued',
    attempt           INTEGER     NOT NULL DEFAULT 1,
    max_attempts      INTEGER     NOT NULL DEFAULT 5,
    initial_backoff_ms BIGINT     NOT NULL DEFAULT 250,
    enqueued_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at        TIMESTAMPTZ,
    finished_at       TIMESTAMPTZ,
    claimed_by        TEXT,
    claimed_at        TIMESTAMPTZ,
    last_error        TEXT,
    run_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Fast path for workers polling for ready jobs
CREATE INDEX IF NOT EXISTS idx_autumn_jobs_ready
    ON autumn_jobs (run_at ASC)
    WHERE status = 'enqueued';

-- Dashboard queries filter by status and finished_at
CREATE INDEX IF NOT EXISTS idx_autumn_jobs_status_finished
    ON autumn_jobs (status, finished_at DESC);

-- Dashboard enqueued-tab sorted by enqueued_at
CREATE INDEX IF NOT EXISTS idx_autumn_jobs_enqueued_dashboard
    ON autumn_jobs (enqueued_at DESC)
    WHERE status = 'enqueued';

-- Stale-claim recovery scans running jobs by claimed_at
CREATE INDEX IF NOT EXISTS idx_autumn_jobs_stale_recovery
    ON autumn_jobs (claimed_at)
    WHERE status = 'running';
