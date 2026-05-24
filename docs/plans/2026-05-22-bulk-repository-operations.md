# Bulk Repository Operations Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Expose high-performance, transaction-safe, and hook-compliant batched/bulk CRUD operations (`save_many`, `update_many`, `delete_many`, `upsert_many`) in the macro-generated `#[repository]` trait and its Postgres implementation.

**Architecture:** 
- The macro-generated `#[repository]` trait is expanded with bulk methods taking slices of plain types.
- If hooks are enabled (`config.hooks_type.is_some()`), `before_` hooks are executed sequentially on each row. For `update_many` and `delete_many`, the original database records are fetched first via `SELECT FOR UPDATE` to construct the hooks context, ensuring full hook fidelity.
- If hooks are enabled, `upsert_many` is rejected at compile-time because determining whether a row will insert vs update is infeasible before database execution.
- Bulk queries are executed using single SQL batch statements chunked under the Postgres parameter ceiling (65,535).
- An opt-in `save_many_skip_invalid` method validates rows via hooks, batch-inserts successes, and falls back to row-by-row insertion on DB constraint violations to return a full list of successes and failures.

**Tech Stack:** Rust 2024, Diesel 2.3, diesel-async 0.8, syn/quote 2.0

---

## User Review Required

> [!IMPORTANT]
> **Compile-Time Safety**: When a repository has hooks configured, `upsert_many` is explicitly rejected at compile time. This prevents silent bypasses of `before_create` / `before_update` hooks, since Postgres decides on-conflict handling at runtime.
> 
> **Database Round Trips with Hooks**: For bulk updates/deletions on hooked repositories, a `SELECT ... FOR UPDATE` is automatically performed first to fetch the current rows so that `before_update` and `before_delete` hooks can inspect the existing state. On repositories without hooks (the default zero-cost path), no extra SELECT is performed.

## Open Questions

None at this time. The requirements are extremely well-specified.

## Proposed Changes

### 1. Model Code Generation

#### [MODIFY] [model.rs](file:///c:/Users/markm/autumn/autumn-macros/src/model.rs)
- Derive `::diesel::Insertable` on `#model_name` (in addition to `New#model_name`) to support `upsert_many` which requires a full model instance containing the primary key.

### 2. Repository Code Generation

#### [MODIFY] [repository.rs](file:///c:/Users/markm/autumn/autumn-macros/src/repository.rs)
- Modify the `repository` macro trait definition to generate:
  ```rust
  fn save_many(&self, new: &[#new_name]) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<Vec<#model_name>>> + Send;
  fn save_many_skip_invalid(&self, new: &[#new_name]) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<(Vec<#model_name>, Vec<(usize, ::autumn_web::AutumnError)>)>> + Send;
  fn update_many(&self, ids: &[i64], changes: &#update_name) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<Vec<#model_name>>> + Send;
  fn delete_many(&self, ids: &[i64]) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<()>> + Send;
  fn upsert_many(&self, records: &[#model_name]) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<Vec<#model_name>>> + Send;
  ```
- Generate implementations for these methods in `Pg#trait_name`:
  - **Chunking Limit**: Ensure chunk sizes are limited to `65535 / columns_count`.
  - **Hook-Free Path**: Direct single SQL execution.
  - **Hooked Path**: Run `before_` hooks, execute DB batch inside transaction, run `after_` hooks, and stage commit hooks. Reject `upsert_many` if hooks are enabled.

---

## Bite-Sized Checklist

### Task 1: Red Phase (Failing Tests)

**Files:**
- Create: `autumn/tests/repository_bulk_operations.rs`

**Step 1: Write the failing tests**
Write a comprehensive suite covering:
- Happy paths for `save_many`, `update_many`, `delete_many`, and `upsert_many` without hooks.
- Hook integration tests for `save_many`, `update_many`, and `delete_many` (validating before/after hooks and transaction rollbacks on hook failure).
- Error handling / rollback tests for partial failures.
- `save_many_skip_invalid` happy paths and database constraint fallback paths.
- Compile-fail test verifying `upsert_many` fails when hooks are configured.

**Step 2: Run tests to verify they fail**
Run: `cargo test --test repository_bulk_operations -- --ignored --test-threads=1`
Expected: Compilation fails due to missing methods on the repository trait.

**Step 3: Commit Red Phase**
```bash
git add autumn/tests/repository_bulk_operations.rs
git commit -m "test: add failing bulk operations integration tests (RED)"
```

### Task 2: Green Phase (Model Codegen & Compile-Fail Verification)

**Files:**
- Modify: `autumn-macros/src/model.rs`
- Modify: `autumn-macros/src/repository.rs`

**Step 1: Implement Model Insertable Derive**
Ensure `#model_name` derives `::diesel::Insertable` so it can be used in `upsert_many`.

**Step 2: Implement Trait Method Signatures and Compile-Time Rejection**
Update `repository.rs` to generate the new methods in the trait definition. If `config.hooks_type.is_some()`, verify that `upsert_many` causes a compile error.

**Step 3: Commit Model Codegen**
```bash
git add autumn-macros/src/model.rs autumn-macros/src/repository.rs
git commit -m "feat: derive Insertable on Model and add compile-time checks (GREEN)"
```

### Task 3: Green Phase (Implement Bulk Methods without Hooks)

**Files:**
- Modify: `autumn-macros/src/repository.rs`

**Step 1: Implement Hook-Free Bulk Bodies**
Generate chunked database actions in `Pg#trait_name` when `config.hooks_type` is `None`:
- `save_many`: Chunk the insert and return concatenated results.
- `update_many`: Generate `diesel::update(#table_ident::table.filter(#table_ident::id.eq_any(chunk))).set(changes)`.
- `delete_many`: Generate `diesel::delete(#table_ident::table.filter(#table_ident::id.eq_any(chunk)))`.
- `upsert_many`: Generate chunked upsert using `insert_into(#table_ident::table).values(chunk).on_conflict(#table_ident::id).do_update().set(excluded_values)`.

**Step 2: Run Tests**
Verify that hook-free tests compile and pass.

**Step 3: Commit Hook-Free Path**
```bash
git commit -am "feat: implement hook-free bulk CRUD operations (GREEN)"
```

### Task 4: Green Phase (Implement Bulk Methods with Hooks)

**Files:**
- Modify: `autumn-macros/src/repository.rs`

**Step 1: Implement Hooked Bulk Bodies**
Generate hook-aware transaction wrapping and execution:
- `save_many`: Sequential `before_create` -> chunked insert -> sequential `after_create` + staged commit hooks.
- `update_many`: SELECT FOR UPDATE -> sequential `before_update` on draft -> execute updates -> sequential `after_update` + staged commit hooks.
- `delete_many`: SELECT FOR UPDATE -> sequential `before_delete` -> execute bulk delete -> sequential `after_delete` + staged commit hooks.
- `save_many_skip_invalid`: Loop over inputs, run `before_create`. Collect successes. Batch-insert successes inside transaction. If batch fails, fall back to row-by-row insert, recording successes and DB errors. Run `after_create` on all successes.

**Step 2: Run Tests**
Verify that hook-enabled tests pass sequentially.

**Step 3: Commit Hooked Path**
```bash
git commit -am "feat: implement hook-aware bulk CRUD operations and skip_invalid fallback (GREEN)"
```

### Task 5: Refactor & Clean Up

**Files:**
- Modify: `autumn-macros/src/repository.rs`
- Modify: `autumn/tests/repository_bulk_operations.rs`

**Step 1: Run formatters and lints**
Run: `cargo fmt` and `cargo clippy --workspace --all-targets`

**Step 2: Commit Refactoring**
```bash
git commit -am "refactor: clean up bulk ORM codegen and formatting"
```

### Task 6: Documentation and Verification

**Files:**
- Modify: `docs/guide/repositories.md`
- Modify: `docs/guide/transactions.md`
- Modify: `examples/todo-app` or similar (add demonstration)

**Step 1: Add documentation**
Add detailed guides on chunk limits, transaction composition, hook lifecycles, and a cookbook entry.

**Step 2: Verify success metrics**
Measure performance difference between sequential `save()` loop vs `save_many()`. Confirm >50x round trip reduction.

---

## Verification Plan

### Automated Tests
- Sequentially run the integration tests:
  ```bash
  cargo test --test repository_bulk_operations -- --ignored --test-threads=1
  ```
- Run the full workspace test suite:
  ```bash
  cargo test --workspace -- --test-threads=1
  ```

### Performance Validation
- Run a benchmark inserting 10,000 records to confirm the round-trip savings.
