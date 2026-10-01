### Added

- **jobs:** Redis dead-letter retention is now configurable via
  `jobs.redis.dead_letter_limit` (`AUTUMN_JOBS__REDIS__DEAD_LETTER_LIMIT`),
  default `1000` (today's behavior); `0` means unbounded (issue #3055).

### Fixed

- **jobs:** the Redis backend's dead-letter trim no longer silently destroys
  forensic evidence: every trim now emits a `warn!` log and increments the
  `autumn_jobs_dead_letter_trimmed_total` counter (scraped from
  `/actuator/prometheus`) (issue #3055).

### Documentation

- **jobs:** documented the dead-letter retention policy for every backend
  (Redis / Postgres / SQLite / local) in the jobs guide (issue #3055).
