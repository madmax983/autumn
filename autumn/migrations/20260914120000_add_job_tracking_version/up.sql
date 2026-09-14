-- Issue #2581: the tracked-job retry compare-and-swap moved off the
-- `updated_at` timestamp onto a write counter, so Postgres needs the same
-- `version` column the SQLite table already carries. A timestamp token
-- repeats when two writes land inside one millisecond (or under a clock that
-- does not advance), letting a stale admin retry reset clobber a fresher
-- terminal write; a counter never repeats.
--
-- Existing rows start at 0; every `PgJobTrackingStore` write bumps the
-- column, and `get` stamps the column value over the JSON blob's copy.
ALTER TABLE autumn_job_tracking ADD COLUMN IF NOT EXISTS version BIGINT NOT NULL DEFAULT 0;
