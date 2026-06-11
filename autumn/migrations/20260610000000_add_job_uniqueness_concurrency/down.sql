DROP INDEX IF EXISTS idx_autumn_jobs_concurrency_running;
DROP INDEX IF EXISTS idx_autumn_jobs_unique_inflight;
ALTER TABLE autumn_jobs DROP COLUMN IF EXISTS concurrency_limit;
ALTER TABLE autumn_jobs DROP COLUMN IF EXISTS concurrency_key;
ALTER TABLE autumn_jobs DROP COLUMN IF EXISTS unique_window;
ALTER TABLE autumn_jobs DROP COLUMN IF EXISTS unique_key;
