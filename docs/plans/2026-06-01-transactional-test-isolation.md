# Transactional Test Isolation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add transactional test isolation to Autumn database integration tests so that DB tests don't leak state between runs.

**Architecture:** 
Implement transactional isolation by adding a dedicated, private connection pool of `max_size(1)` for each transactional `TestApp`. We install a `TransactionalDbInterceptor` on the pool that starts a transaction (`BEGIN`) on the first checkout. When the test finishes and the `TestClient` is dropped, the pool is dropped, the connection is closed, and PostgreSQL automatically rolls back the transaction.

**Tech Stack:** Rust, diesel-async, deadpool, testcontainers

---

## User Review Required

> [!IMPORTANT]
> Transactional test isolation requires a dedicated, private connection pool of `max_size(1)` for each test. This ensures that all database operations within the test and HTTP handlers run within the same PostgreSQL transaction and can see each other's uncommitted changes. Concurrent database operations within the same test are queued sequentially.

---

## Proposed Changes

### Component: Autumn Web Testing utilities

#### [MODIFY] [test.rs](file:///c:/Users/markm/autumn/autumn/src/test.rs)

1. Add `transactional: bool` and `transactional_url: Option<String>` to `TestApp`.
2. Add `TestApp::transactional(mut self) -> Self` to enable transactional isolation using the configured database URL.
3. Add `TestApp::with_transactional_db(mut self, url: impl Into<String>) -> Self` to enable transactional isolation with an explicit database URL.
4. Modify `TestApp::build` to:
   - If transactional isolation is enabled:
     - Resolve the database URL (either from `transactional_url` or from `self.config.database.effective_primary_url()`).
     - Build a private pool of `max_size(1)` using that URL.
     - Register the `TransactionalDbInterceptor` as a DB connection interceptor.
     - Set this pool as the primary pool on `AppState`.
5. Implement the `TransactionalDbInterceptor` in `test.rs` or `db.rs`.

---

## Verification Plan

### Automated Tests
- Create a new integration test file [transactional_test_integration.rs](file:///c:/Users/markm/autumn/autumn/tests/transactional_test_integration.rs).
- Implement two tests that run against the same Postgres container:
  - `test_1_insert_without_cleanup`: Inserts an item into `test_items` table and does not clean up.
  - `test_2_verify_isolation`: Verifies that the `test_items` table is completely empty, proving that `test_1`'s changes were rolled back and did not leak.
- Run the new integration tests with:
  ```bash
  cargo test --test transactional_test_integration -- --include-ignored
  ```

### Manual Verification
- Verify that standard tests in [test_db_integration.rs](file:///c:/Users/markm/autumn/autumn/tests/test_db_integration.rs) continue to pass.
