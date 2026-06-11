-- Additive columns for #[job] uniqueness keys and concurrency limits (#829).
-- All columns are nullable so existing rows and jobs without the new
-- attributes behave exactly as before.
ALTER TABLE autumn_jobs ADD COLUMN IF NOT EXISTS unique_key        TEXT;
ALTER TABLE autumn_jobs ADD COLUMN IF NOT EXISTS unique_window     TEXT;
ALTER TABLE autumn_jobs ADD COLUMN IF NOT EXISTS concurrency_key   TEXT;
ALTER TABLE autumn_jobs ADD COLUMN IF NOT EXISTS concurrency_limit INTEGER;

-- Distributed dedup backstop: at most one in-flight job per (name, unique_key).
-- Enqueue uses ON CONFLICT DO NOTHING against this index, so two racing
-- enqueues across app instances coalesce to a single row.
CREATE UNIQUE INDEX IF NOT EXISTS idx_autumn_jobs_unique_inflight
    ON autumn_jobs (name, unique_key)
    WHERE unique_key IS NOT NULL AND status IN ('enqueued', 'running');

-- Claim-time concurrency checks count running jobs per (name, concurrency_key).
CREATE INDEX IF NOT EXISTS idx_autumn_jobs_concurrency_running
    ON autumn_jobs (name, concurrency_key)
    WHERE status = 'running';
