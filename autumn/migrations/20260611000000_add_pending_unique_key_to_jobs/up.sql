-- Preserve the unique_key across the claim→timeout→stale-recovery round-trip
-- for jobs with unique_window = 'pending'.
--
-- When a pending-window job is claimed the dedup key is cleared (unique_key = NULL)
-- so a fresh duplicate can be enqueued while the job runs.  If the worker dies the
-- stale-recovery path re-enqueues the row without the key, silently breaking dedup.
-- This column lets the claim SQL save the key before clearing it; stale recovery
-- reads it back and restores unique_key on the re-enqueued row.
ALTER TABLE autumn_jobs ADD COLUMN IF NOT EXISTS pending_unique_key TEXT;
