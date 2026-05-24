# Repositories & Bulk Operations

Repositories in `autumn-web` provide a clean, type-safe, and highly optimized ORM-like data access layer. By annotating a trait with `#[autumn_web::repository(Model, table = "table_name")]`, Autumn automatically generates high-performance implementations targeting PostgreSQL using `diesel-async`.

In version `0.4.0`, Autumn introduces high-performance **Bulk CRUD operations** to minimize database round trips and execute massive writes transaction-safely and hook-compliantly.

---

## Generated Bulk CRUD Methods

When you declare a repository, the generated `Pg[Name]Repository` automatically implements the following high-performance bulk operations:

```rust
fn save_many(
    &self, 
    new: &[NewModel]
) -> impl Future<Output = AutumnResult<Vec<Model>>> + Send;

fn save_many_skip_invalid(
    &self, 
    new: &[NewModel]
) -> impl Future<Output = AutumnResult<(Vec<Model>, Vec<(usize, AutumnError)>)>> + Send;

fn update_many(
    &self, 
    ids: &[i64], 
    changes: &UpdateModel
) -> impl Future<Output = AutumnResult<Vec<Model>>> + Send;

fn delete_many(
    &self, 
    ids: &[i64]
) -> impl Future<Output = AutumnResult<()>> + Send;

fn upsert_many(
    &self, 
    records: &[Model]
) -> impl Future<Output = AutumnResult<Vec<Model>>> + Send;
```

---

## 1. High-Performance Batch Insertion: `save_many`

`save_many` takes a slice of new records and inserts them in a single batch statement.

### Non-Hooked (Zero-Cost Path)
If your model has no hooks configured, `save_many` translates to a single SQL query:
```sql
INSERT INTO table_name (col1, col2, ...) 
VALUES ($1, $2, ...), ($3, $4, ...), ... 
RETURNING *;
```
For large inputs, queries are automatically chunked under the Postgres parameter ceiling (65,535 parameters), preventing compilation or runtime DB overflow errors.

### Hook-Aware Execution
If hooks are enabled on your repository, `save_many` guarantees full transaction integrity:
1. Runs `before_create` hooks **sequentially** on each record.
2. Batches the validated records and inserts them in a single database round trip inside a transaction.
3. Runs `after_create` hooks sequentially on successfully inserted records.
4. Stages `after_create_commit` hooks to fire only after the surrounding transaction successfully commits.

---

## 2. Validation & Partial Success: `save_many_skip_invalid`

When bulk importing dirty external data (e.g., from CSVs or public API hooks), some rows might violate business rules or database constraints. `save_many_skip_invalid` enables maximum throughput without losing valid rows.

- It runs `before_create` hooks on each row and filters out custom validation failures.
- It attempts a high-speed batch insert of all successful records in a transaction.
- **Constraint Fallback**: If the batch insert fails due to a database constraint (e.g., `UniqueViolation`), it automatically falls back to row-by-row insertion for that chunk, isolating individual DB constraint failures.
- Returns a tuple of `(successful_models, list_of_errors_with_indices)`.

---

## 3. Bulk Updates: `update_many`

`update_many` modifies a batch of records identified by their IDs in a single SQL operation.

### Non-Hooked
Updates all matching rows directly:
```rust
repo.update_many(&[1, 2, 3], &UpdatePost { title: Some("Bulk Updated Title".to_string()) }).await?;
```

### Hook-Aware
If `before_update` hooks are configured:
1. Performs a `SELECT ... FOR UPDATE` on all specified IDs to load their current state.
2. For each row, constructs an `UpdateDraft` containing the original model and applies the changes.
3. Runs `before_update` hooks on each draft.
4. Updates all matching records in the database.
5. Runs `after_update` hooks.

---

## 4. Bulk Deletions: `delete_many`

`delete_many` deletes or soft-deletes a batch of records in a single statement.

### Non-Hooked
Runs a single direct delete or soft-delete update statement.

### Hook-Aware
1. Performs a `SELECT ... FOR UPDATE` on all specified IDs.
2. Runs `before_delete` hooks sequentially.
3. Executes the batch delete / soft-delete.
4. Runs `after_delete` hooks sequentially.

---

## 5. Bulk Upserts: `upsert_many`

`upsert_many` executes high-performance "insert-or-update" operations using a single SQL query matching on the primary key:
```sql
INSERT INTO table_name (id, col1, col2, ...) 
VALUES ($1, $2, ...), ($3, $4, ...) 
ON CONFLICT (id) DO UPDATE SET col1 = EXCLUDED.col1, ... 
RETURNING *;
```

> [!IMPORTANT]
> **Compile-Time Hook Safety**: If hooks are enabled on your repository, calling `upsert_many` is explicitly **rejected at compile-time**. 
> Because Postgres determines whether a row will insert vs update at runtime, it is impossible to correctly invoke `before_create` or `before_update` hooks before sending the query. To prevent silent hook bypass, this is caught during compilation.

---

## Performance & Scaling Guidelines

Bulk operations are built for maximum performance, with the following built-in safeguards:

### The Postgres Parameter Ceiling
Postgres supports a maximum of 65,535 parameters per statement. If you try to insert 10,000 rows with 8 columns, that requires 80,000 parameters, which ordinarily crashes.
Autumn automatically calculates the optimal chunk size based on your model's columns and inserts in chunks (e.g. 1000 records at a time) to always remain well below the ceiling while maintaining peak batching throughput (>50x speedups over individual insertions).
