1. **Analyze Mutants:** The `cargo mutants` output shows several mutants in `autumn/src/db.rs` for the `is_query_canceled` function.
2. **Missing Tests:** The `is_query_canceled` function currently has no tests covering its different branches (string matching, downcasting to `tokio_postgres::Error`, and downcasting to `tokio_postgres::error::DbError`). This is a `MISSING_COVERAGE` gap.
3. **Write Kill Shot:** Add a test module for `is_query_canceled` in `autumn/src/db.rs` within the `tests` module. Create tests to cover:
   - String matching variants (e.g. "57014", "query_canceled", "statement timeout")
   - Downcasting to `tokio_postgres::Error` with `SqlState::QUERY_CANCELED`
   - Downcasting to `tokio_postgres::error::DbError` with `SqlState::QUERY_CANCELED`
   - Negative cases where it's not a query canceled error.
4. **Verify:** Run `cargo mutants --iterate --file autumn/src/db.rs --timeout 60` to confirm the mutants are killed.
5. **Pre-commit Checks:** Complete pre commit steps to ensure proper testing, verification, review, and reflection are done.
6. **Submit PR:** Submit a PR with the required Sentry persona format.
