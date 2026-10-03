### Fixed

- **custom-domains:** three issuance-budget defects (issue #3102). A spent
  budget no longer charges the domain a failure: the deferral parks it until
  the budget's own window rolls without touching `consecutive_failures`, so a
  domain examined while the global budget is full can no longer climb to the
  24-hour maximum backoff. The orchestrator samples the clock fresh at each
  decision point instead of stamping a whole tick with its start time, so a
  slow order can no longer land a failure's `next_attempt_unix` in the past
  (which the next tick read as "due now", spending quota instead of backing
  off). And every order's attempt timestamp is persisted on the domain record,
  so a restart inside the budget window rebuilds the per-domain and global
  windows instead of forgetting every order already placed and spending the
  shared ACME account quota again.
