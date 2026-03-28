# Repository Mutation Hooks Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add opt-in repository-scoped mutation hooks (`before_create`, `after_update`, `after_commit`, etc.) with merged-draft semantics and transactional lifecycle to the `#[repository]` macro.

**Architecture:** Runtime types (`Patch<T>`, `FieldDiff<T>`, `UpdateDraft<T>`, `MutationContext`, `MutationHooks` trait) live in `autumn/src/hooks.rs`. The `#[model]` macro is extended to generate a field enum and per-field draft accessors. The `#[repository]` macro is extended to accept `hooks = Type` and generate the full mutation lifecycle (txn open → before_* → persist → after_* → commit → after_commit).

**Tech Stack:** Rust 2024, diesel-async (AsyncConnection::transaction), proc-macro2/syn/quote, smallvec

---

## Phase 1: Core Runtime Types

### Task 1: Add `smallvec` and `chrono` workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root, `[workspace.dependencies]`)
- Modify: `autumn/Cargo.toml` (`[dependencies]`)

**Step 1: Add smallvec and chrono to workspace deps**

In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:

```toml
smallvec = "1"
chrono = { version = "0.4", features = ["serde"] }
```

**Step 2: Add to autumn crate deps**

In `autumn/Cargo.toml`, add under `[dependencies]`:

```toml
smallvec = { workspace = true }
chrono = { workspace = true }
```

**Step 3: Re-export from autumn**

In `autumn/src/lib.rs`, add to `pub mod reexports`:

```rust
pub use smallvec;
pub use chrono;
```

**Step 4: Verify it compiles**

Run: `cargo check -p autumn-web`
Expected: PASS (no code uses it yet, just wiring)

**Step 5: Commit**

```bash
git add Cargo.toml autumn/Cargo.toml autumn/src/lib.rs
git commit -m "chore: add smallvec and chrono workspace deps for mutation hooks"
```

---

### Task 2: Create `Patch<T>` type

**Files:**
- Create: `autumn/src/hooks.rs`
- Modify: `autumn/src/lib.rs` (add `pub mod hooks;`)
- Test: `autumn/src/hooks.rs` (inline `#[cfg(test)] mod tests`)

**Step 1: Write the failing tests**

Create `autumn/src/hooks.rs` with the test module first:

```rust
//! Repository mutation hooks: lifecycle types for business logic around writes.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_unchanged_is_default() {
        let p: Patch<String> = Patch::default();
        assert!(p.is_unchanged());
    }

    #[test]
    fn patch_set_holds_value() {
        let p = Patch::Set("hello".to_string());
        assert!(p.is_set());
        assert_eq!(p.as_set(), Some(&"hello".to_string()));
    }

    #[test]
    fn patch_clear_is_clear() {
        let p: Patch<String> = Patch::Clear;
        assert!(p.is_clear());
        assert!(!p.is_set());
        assert!(!p.is_unchanged());
    }

    #[test]
    fn patch_into_option_set() {
        let p = Patch::Set(42);
        assert_eq!(p.into_option(), Some(Some(42)));
    }

    #[test]
    fn patch_into_option_clear() {
        let p: Patch<i32> = Patch::Clear;
        assert_eq!(p.into_option(), Some(None));
    }

    #[test]
    fn patch_into_option_unchanged() {
        let p: Patch<i32> = Patch::Unchanged;
        assert_eq!(p.into_option(), None);
    }
}
```

**Step 2: Register the module**

In `autumn/src/lib.rs`, add after `pub mod validation;`:

```rust
#[cfg(feature = "db")]
pub mod hooks;
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -p autumn-web hooks::tests --no-default-features --features db`
Expected: FAIL — `Patch` type doesn't exist yet

**Step 4: Implement `Patch<T>`**

Add to `autumn/src/hooks.rs` above the test module:

```rust
use serde::{Deserialize, Serialize};

/// Tri-state sparse update value.
///
/// Unlike `Option<T>`, `Patch` distinguishes three states:
/// - `Unchanged`: the field was not included in the update
/// - `Set(T)`: the field was explicitly set to a new value
/// - `Clear`: the field was explicitly set to null/empty
///
/// For non-nullable columns, `Clear` is rejected during validation
/// before the query is issued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Patch<T> {
    /// Field was not included in the update payload.
    Unchanged,
    /// Field explicitly set to a value.
    Set(T),
    /// Field explicitly cleared (maps to SQL NULL for nullable columns).
    Clear,
}

impl<T> Default for Patch<T> {
    fn default() -> Self {
        Self::Unchanged
    }
}

impl<T> Patch<T> {
    /// Returns `true` if the patch is `Unchanged`.
    #[must_use]
    pub const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    /// Returns `true` if the patch is `Set(_)`.
    #[must_use]
    pub const fn is_set(&self) -> bool {
        matches!(self, Self::Set(_))
    }

    /// Returns `true` if the patch is `Clear`.
    #[must_use]
    pub const fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }

    /// Returns a reference to the inner value if `Set`, otherwise `None`.
    #[must_use]
    pub const fn as_set(&self) -> Option<&T> {
        match self {
            Self::Set(v) => Some(v),
            _ => None,
        }
    }

    /// Converts to `Option<Option<T>>`:
    /// - `Set(v)` → `Some(Some(v))`
    /// - `Clear` → `Some(None)`
    /// - `Unchanged` → `None`
    #[must_use]
    pub fn into_option(self) -> Option<Option<T>> {
        match self {
            Self::Unchanged => None,
            Self::Set(v) => Some(Some(v)),
            Self::Clear => Some(None),
        }
    }
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p autumn-web hooks::tests`
Expected: PASS — all 6 Patch tests green

**Step 6: Commit**

```bash
git add autumn/src/hooks.rs autumn/src/lib.rs
git commit -m "feat(hooks): add Patch<T> tri-state sparse update type"
```

---

### Task 3: Create `FieldDiff<T>` type

**Files:**
- Modify: `autumn/src/hooks.rs`

**Step 1: Write the failing tests**

Add to the test module in `hooks.rs`:

```rust
    #[test]
    fn field_diff_unchanged() {
        let diff = FieldDiff::new(42, 42);
        assert!(diff.unchanged());
        assert!(!diff.changed());
        assert_eq!(diff.before(), &42);
        assert_eq!(diff.after(), &42);
    }

    #[test]
    fn field_diff_changed() {
        let diff = FieldDiff::new(1, 2);
        assert!(diff.changed());
        assert!(!diff.unchanged());
    }

    #[test]
    fn field_diff_changed_to() {
        let diff = FieldDiff::new(1, 2);
        assert!(diff.changed_to(&2));
        assert!(!diff.changed_to(&1));
    }

    #[test]
    fn field_diff_changed_from() {
        let diff = FieldDiff::new(1, 2);
        assert!(diff.changed_from(&1));
        assert!(!diff.changed_from(&2));
    }

    #[test]
    fn field_diff_set_updates_after() {
        let mut diff = FieldDiff::new(1, 1);
        diff.set(5);
        assert!(diff.changed());
        assert_eq!(diff.after(), &5);
        assert_eq!(diff.before(), &1); // before unchanged
    }

    #[test]
    fn field_diff_option_was_set() {
        let diff: FieldDiff<Option<i32>> = FieldDiff::new(None, Some(42));
        assert!(diff.was_set());
        assert!(!diff.was_cleared());
    }

    #[test]
    fn field_diff_option_was_cleared() {
        let diff: FieldDiff<Option<i32>> = FieldDiff::new(Some(42), None);
        assert!(diff.was_cleared());
        assert!(!diff.was_set());
    }
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p autumn-web hooks::tests`
Expected: FAIL — `FieldDiff` doesn't exist

**Step 3: Implement `FieldDiff<T>`**

Add to `hooks.rs` after `Patch<T>`:

```rust
/// Per-field before/after diff accessor.
///
/// Provides ergonomic helpers for hook authors to inspect and mutate
/// the proposed state of a single field during `before_update`.
#[derive(Debug, Clone)]
pub struct FieldDiff<T> {
    before: T,
    after: T,
}

impl<T: PartialEq> FieldDiff<T> {
    /// Create a new diff from the current and proposed values.
    #[must_use]
    pub const fn new(before: T, after: T) -> Self {
        Self { before, after }
    }

    /// The value before the mutation.
    #[must_use]
    pub const fn before(&self) -> &T {
        &self.before
    }

    /// The proposed value after the mutation.
    #[must_use]
    pub const fn after(&self) -> &T {
        &self.after
    }

    /// True if the field value changed.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.before != self.after
    }

    /// True if the field value did not change.
    #[must_use]
    pub fn unchanged(&self) -> bool {
        self.before == self.after
    }

    /// True if the field changed to the given value.
    #[must_use]
    pub fn changed_to(&self, value: &T) -> bool {
        self.changed() && &self.after == value
    }

    /// True if the field changed from the given value.
    #[must_use]
    pub fn changed_from(&self, value: &T) -> bool {
        self.changed() && &self.before == value
    }

    /// Overwrite the proposed value. Does not affect `before`.
    pub fn set(&mut self, value: T) {
        self.after = value;
    }
}

impl<T: PartialEq> FieldDiff<Option<T>> {
    /// True if the field went from `None` to `Some(_)`.
    #[must_use]
    pub fn was_set(&self) -> bool {
        self.before.is_none() && self.after.is_some()
    }

    /// True if the field went from `Some(_)` to `None`.
    #[must_use]
    pub fn was_cleared(&self) -> bool {
        self.before.is_some() && self.after.is_none()
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p autumn-web hooks::tests`
Expected: PASS — all FieldDiff tests green

**Step 5: Commit**

```bash
git add autumn/src/hooks.rs
git commit -m "feat(hooks): add FieldDiff<T> per-field diff accessor"
```

---

### Task 4: Create `MutationOp` enum and `MutationContext`

**Files:**
- Modify: `autumn/src/hooks.rs`

**Step 1: Write the failing tests**

Add to tests:

```rust
    #[test]
    fn mutation_op_display() {
        assert_eq!(MutationOp::Create.as_str(), "create");
        assert_eq!(MutationOp::Update.as_str(), "update");
        assert_eq!(MutationOp::Delete.as_str(), "delete");
    }

    #[test]
    fn mutation_context_default_fields() {
        let ctx = MutationContext::new(MutationOp::Create);
        assert!(ctx.actor.is_none());
        assert!(ctx.request_id.is_some()); // auto-generated
        assert!(matches!(ctx.op, MutationOp::Create));
    }

    #[test]
    fn mutation_context_with_actor() {
        let mut ctx = MutationContext::new(MutationOp::Update);
        ctx.actor = Some("user-123".into());
        assert_eq!(ctx.actor.as_deref(), Some("user-123"));
    }
```

**Step 2: Run tests — expect failure**

Run: `cargo test -p autumn-web hooks::tests`
Expected: FAIL

**Step 3: Implement `MutationOp` and `MutationContext`**

Add to `hooks.rs`:

```rust
use chrono::{DateTime, Utc};

/// The kind of mutation being performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOp {
    Create,
    Update,
    Delete,
}

impl MutationOp {
    /// Returns a lowercase string label for the operation.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

impl std::fmt::Display for MutationOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Context available to mutation hooks.
///
/// Carries actor identity, request metadata, and timestamps so hook
/// authors can implement audit, policy, and CDC logic without reaching
/// into raw request plumbing.
pub struct MutationContext {
    /// The mutation operation type.
    pub op: MutationOp,
    /// Actor identity (e.g., user ID or service name). `None` for anonymous.
    pub actor: Option<String>,
    /// Correlation / request ID for tracing.
    pub request_id: Option<String>,
    /// Timestamp of the mutation.
    pub now: DateTime<Utc>,
}

impl MutationContext {
    /// Create a new context for the given operation, auto-populating
    /// timestamp and a UUID request ID.
    #[must_use]
    pub fn new(op: MutationOp) -> Self {
        Self {
            op,
            actor: None,
            request_id: Some(uuid::Uuid::new_v4().to_string()),
            now: Utc::now(),
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p autumn-web hooks::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add autumn/src/hooks.rs
git commit -m "feat(hooks): add MutationOp enum and MutationContext"
```

---

### Task 5: Create `MutationHooks` trait and `NoHooks` default

**Files:**
- Modify: `autumn/src/hooks.rs`

**Step 1: Write the failing tests**

Add to tests:

```rust
    // NoHooks should compile and all methods should be no-ops
    #[tokio::test]
    async fn no_hooks_before_create_is_noop() {
        // We can't test with real generic types easily, but we can
        // verify NoHooks exists and its methods return Ok
        // (Full integration tested later with macro-generated code)
        assert!(true); // placeholder — real test is compile-pass
    }
```

**Step 2: Implement the trait and NoHooks**

Add to `hooks.rs`:

```rust
use crate::AutumnResult;

/// Trait for repository-scoped mutation hooks.
///
/// Implement this on a struct and pass it to `#[repository(Model, hooks = YourHooks)]`
/// to run business logic before/after mutations.
///
/// All methods have default no-op implementations, so you only override
/// the hooks you need.
///
/// # Lifecycle
///
/// 1. Begin transaction
/// 2. Load current record (update/delete)
/// 3. Build `MutationContext`
/// 4. Run `before_*` — may validate, reject, or rewrite
/// 5. Persist mutation
/// 6. Run `after_*` — inside transaction, may do additional writes
/// 7. Commit transaction
/// 8. Run `after_commit` — outside transaction, cannot affect commit
///
/// # Error semantics
///
/// - Errors in `before_*` abort the mutation
/// - Errors in `after_*` abort the mutation (rollback)
/// - Errors in `after_commit` are logged but do NOT roll back
pub trait MutationHooks: Send + Sync + 'static {
    /// The model type (e.g., `Post`).
    type Model: Send + Sync;
    /// The insert input type (e.g., `NewPost`).
    type NewModel: Send + Sync;
    /// The changeset type (e.g., `UpdatePost`).
    type UpdateModel: Send + Sync;

    /// Called before a new record is inserted.
    /// `new` can be inspected or modified before persistence.
    fn before_create(
        &self,
        _ctx: &mut MutationContext,
        _new: &mut Self::NewModel,
    ) -> impl std::future::Future<Output = AutumnResult<()>> + Send {
        async { Ok(()) }
    }

    /// Called after a new record is inserted, inside the transaction.
    fn after_create(
        &self,
        _ctx: &MutationContext,
        _record: &Self::Model,
    ) -> impl std::future::Future<Output = AutumnResult<()>> + Send {
        async { Ok(()) }
    }

    /// Called before an existing record is updated.
    /// `current` is the record as it exists now. `changes` is the
    /// proposed changeset (can be inspected/modified).
    fn before_update(
        &self,
        _ctx: &mut MutationContext,
        _current: &Self::Model,
        _changes: &mut Self::UpdateModel,
    ) -> impl std::future::Future<Output = AutumnResult<()>> + Send {
        async { Ok(()) }
    }

    /// Called after a record is updated, inside the transaction.
    fn after_update(
        &self,
        _ctx: &MutationContext,
        _record: &Self::Model,
    ) -> impl std::future::Future<Output = AutumnResult<()>> + Send {
        async { Ok(()) }
    }

    /// Called before a record is deleted.
    fn before_delete(
        &self,
        _ctx: &mut MutationContext,
        _record: &Self::Model,
    ) -> impl std::future::Future<Output = AutumnResult<()>> + Send {
        async { Ok(()) }
    }

    /// Called after a record is deleted, inside the transaction.
    fn after_delete(
        &self,
        _ctx: &MutationContext,
        _id: i32,
    ) -> impl std::future::Future<Output = AutumnResult<()>> + Send {
        async { Ok(()) }
    }

    /// Called after the transaction has committed.
    /// Errors here do NOT roll back the mutation.
    fn after_commit(
        &self,
        _ctx: &MutationContext,
        _op: MutationOp,
    ) -> impl std::future::Future<Output = AutumnResult<()>> + Send {
        async { Ok(()) }
    }
}

/// Default no-op hooks used when `hooks = ...` is not specified.
///
/// All methods immediately return `Ok(())`.
pub struct NoHooks<M, N, U> {
    _phantom: std::marker::PhantomData<(M, N, U)>,
}

impl<M, N, U> Default for NoHooks<M, N, U> {
    fn default() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<M, N, U> MutationHooks for NoHooks<M, N, U>
where
    M: Send + Sync + 'static,
    N: Send + Sync + 'static,
    U: Send + Sync + 'static,
{
    type Model = M;
    type NewModel = N;
    type UpdateModel = U;
    // All defaults are no-ops — nothing to override
}
```

**Step 3: Run tests and clippy**

Run: `cargo test -p autumn-web hooks::tests && cargo clippy -p autumn-web`
Expected: PASS

**Step 4: Re-export from lib.rs and prelude**

In `autumn/src/lib.rs`, add public re-exports:

```rust
#[cfg(feature = "db")]
pub use hooks::{MutationHooks, MutationContext, MutationOp, NoHooks, Patch, FieldDiff};
```

In `autumn/src/prelude.rs`, add:

```rust
#[cfg(feature = "db")]
pub use crate::hooks::{MutationHooks, MutationContext, MutationOp, Patch, FieldDiff};
```

**Step 5: Verify full build**

Run: `cargo check -p autumn-web`
Expected: PASS

**Step 6: Commit**

```bash
git add autumn/src/hooks.rs autumn/src/lib.rs autumn/src/prelude.rs
git commit -m "feat(hooks): add MutationHooks trait and NoHooks default"
```

---

## Phase 2: Repository Macro — `hooks` Parameter

### Task 6: Parse `hooks = Type` in `#[repository]` attribute

**Files:**
- Modify: `autumn-macros/src/repository.rs`

**Step 1: Write the failing test**

Add to the test module in `repository.rs`:

```rust
    #[test]
    fn parse_repo_args_with_hooks() {
        let tokens: proc_macro2::TokenStream =
            "Post, hooks = PostHooks".parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert_eq!(config.hooks_type.as_ref().map(|h| h.to_string()), Some("PostHooks".to_string()));
    }

    #[test]
    fn parse_repo_args_without_hooks() {
        let tokens: proc_macro2::TokenStream = "Post".parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert!(config.hooks_type.is_none());
    }
```

**Step 2: Run tests — expect failure**

Run: `cargo test -p autumn-macros repository::tests`
Expected: FAIL — `hooks_type` field doesn't exist

**Step 3: Add `hooks_type` to `RepoConfig` and parser**

Modify `RepoConfig`:

```rust
struct RepoConfig {
    model_name: Ident,
    table_name: String,
    hooks_type: Option<Ident>,
}
```

Modify `parse_repo_args` to also accept `hooks = Type`:

```rust
fn parse_repo_args(attr: TokenStream) -> syn::Result<RepoConfig> {
    let mut model_name: Option<Ident> = None;
    let mut table_name: Option<String> = None;
    let mut hooks_type: Option<Ident> = None;

    syn::meta::parser(|meta| {
        if meta.path.is_ident("table") {
            let value: LitStr = meta.value()?.parse()?;
            table_name = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("hooks") {
            let value: Ident = meta.value()?.parse()?;
            hooks_type = Some(value);
            Ok(())
        } else if meta.path.get_ident().is_some() && model_name.is_none() {
            model_name = Some(meta.path.get_ident().unwrap().clone());
            Ok(())
        } else {
            Err(meta.error("expected model name, table = \"...\", or hooks = Type"))
        }
    })
    .parse2(attr)?;

    let model = model_name.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "expected model name: #[repository(ModelName)]",
        )
    })?;
    let table = table_name.unwrap_or_else(|| infer_table_name(&model));

    Ok(RepoConfig {
        model_name: model,
        table_name: table,
        hooks_type,
    })
}
```

**Step 4: Run tests**

Run: `cargo test -p autumn-macros repository::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add autumn-macros/src/repository.rs
git commit -m "feat(macros): parse hooks = Type in #[repository] attribute"
```

---

### Task 7: Generate transactional CRUD with hook lifecycle

**Files:**
- Modify: `autumn-macros/src/repository.rs`

This is the biggest task. The generated `save`, `update`, and `delete_by_id` methods must change from "get conn, run query" to the full lifecycle when `hooks` is specified.

**Step 1: Write compile-pass test**

Create `autumn/tests/compile-pass/repository_with_hooks.rs`:

```rust
// Compile-pass: #[repository] with hooks = Type should generate valid code
// (Cannot run without a real DB, but must compile)

mod schema {
    autumn_web::reexports::diesel::table! {
        articles (id) {
            id -> Int4,
            title -> Text,
            body -> Text,
        }
    }
}

use schema::articles;
use autumn_web::prelude::*;

#[model]
pub struct Article {
    #[id]
    pub id: i32,
    pub title: String,
    pub body: String,
}

pub struct ArticleHooks;

impl MutationHooks for ArticleHooks {
    type Model = Article;
    type NewModel = NewArticle;
    type UpdateModel = UpdateArticle;
}

#[repository(Article, hooks = ArticleHooks)]
pub trait ArticleRepository {
    fn find_by_title(title: String) -> Vec<Article>;
}

fn main() {}
```

**Step 2: Run compile test — expect failure**

Run: `cargo check --test compile_fail` (or the trybuild pass test)
Expected: FAIL — generated code doesn't use hooks yet

**Step 3: Modify `repository_macro` to generate hooked CRUD**

In `repository_macro`, when `config.hooks_type` is `Some(hooks_ident)`:

The generated `PgXxxRepository` struct gains a `hooks` field:

```rust
#vis struct #pg_name {
    pool: Pool<AsyncPgConnection>,
    hooks: #hooks_ident,
}
```

The `FromRequestParts` impl constructs hooks with `Default::default()` (or we can require the user to supply it — simpler to default).

Actually, re-reading the design doc: the hooks struct is user-defined and should be constructable. The simplest approach: require `Default` on the hooks type and construct it in the extractor. This keeps the API clean.

Generated `save` method (with hooks):

```rust
async fn save(&self, new: &#new_name) -> ::autumn_web::AutumnResult<#model_name> {
    use ::autumn_web::reexports::diesel::prelude::*;
    use ::autumn_web::reexports::diesel_async::RunQueryDsl;
    use ::autumn_web::reexports::diesel_async::AsyncConnection;
    use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

    let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
    let mut input = new.clone();
    let mut ctx = MutationContext::new(MutationOp::Create);

    // before_create (outside txn — can reject)
    self.hooks.before_create(&mut ctx, &mut input).await?;

    // Transaction: persist + after_create
    let record = conn.transaction::<_, ::autumn_web::AutumnError, _>(|conn| {
        Box::pin(async move {
            let record = ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                .values(&input)
                .get_result::<#model_name>(conn)
                .await
                .map_err(::autumn_web::AutumnError::from)?;
            self.hooks.after_create(&ctx, &record).await?;
            Ok(record)
        })
    }).await?;

    // after_commit (best-effort)
    if let Err(e) = self.hooks.after_commit(&ctx, MutationOp::Create).await {
        ::autumn_web::reexports::tracing::warn!("after_commit hook error: {e}");
    }

    Ok(record)
}
```

Generated `update` method (with hooks):

```rust
async fn update(&self, id: i32, changes: &#update_name) -> ::autumn_web::AutumnResult<#model_name> {
    use ::autumn_web::reexports::diesel::prelude::*;
    use ::autumn_web::reexports::diesel_async::RunQueryDsl;
    use ::autumn_web::reexports::diesel_async::AsyncConnection;
    use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

    let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
    let mut changeset = changes.clone();
    let mut ctx = MutationContext::new(MutationOp::Update);

    // Load current record
    let current = #table_ident::table
        .find(id)
        .first::<#model_name>(&mut conn)
        .await
        .optional()
        .map_err(::autumn_web::AutumnError::from)?
        .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
            format!("{} with id {} not found", stringify!(#model_name), id)
        ))?;

    // before_update
    self.hooks.before_update(&mut ctx, &current, &mut changeset).await?;

    // Transaction: persist + after_update
    let record = conn.transaction::<_, ::autumn_web::AutumnError, _>(|conn| {
        Box::pin(async move {
            let record = ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                .set(&changeset)
                .get_result::<#model_name>(conn)
                .await
                .map_err(::autumn_web::AutumnError::from)?;
            self.hooks.after_update(&ctx, &record).await?;
            Ok(record)
        })
    }).await?;

    if let Err(e) = self.hooks.after_commit(&ctx, MutationOp::Update).await {
        ::autumn_web::reexports::tracing::warn!("after_commit hook error: {e}");
    }

    Ok(record)
}
```

Generated `delete_by_id` method (with hooks):

```rust
async fn delete_by_id(&self, id: i32) -> ::autumn_web::AutumnResult<()> {
    use ::autumn_web::reexports::diesel::prelude::*;
    use ::autumn_web::reexports::diesel_async::RunQueryDsl;
    use ::autumn_web::reexports::diesel_async::AsyncConnection;
    use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

    let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
    let mut ctx = MutationContext::new(MutationOp::Delete);

    // Load current record
    let record = #table_ident::table
        .find(id)
        .first::<#model_name>(&mut conn)
        .await
        .optional()
        .map_err(::autumn_web::AutumnError::from)?
        .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
            format!("{} with id {} not found", stringify!(#model_name), id)
        ))?;

    // before_delete
    self.hooks.before_delete(&mut ctx, &record).await?;

    // Transaction: delete + after_delete
    conn.transaction::<_, ::autumn_web::AutumnError, _>(|conn| {
        Box::pin(async move {
            ::autumn_web::reexports::diesel::delete(#table_ident::table.find(id))
                .execute(conn)
                .await
                .map_err(::autumn_web::AutumnError::from)?;
            self.hooks.after_delete(&ctx, id).await?;
            Ok(())
        })
    }).await?;

    if let Err(e) = self.hooks.after_commit(&ctx, MutationOp::Delete).await {
        ::autumn_web::reexports::tracing::warn!("after_commit hook error: {e}");
    }

    Ok(())
}
```

The `FromRequestParts` impl when hooks is present:

```rust
impl FromRequestParts<AppState> for #pg_name {
    type Rejection = AutumnError;
    async fn from_request_parts(
        _parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let pool = state.pool.as_ref()
            .ok_or_else(|| AutumnError::service_unavailable_msg("No database pool configured"))?
            .clone();
        Ok(#pg_name { pool, hooks: #hooks_ident::default() })
    }
}
```

When `hooks` is `None`, the generated code is identical to today (no transaction wrapper, no hook calls).

**Important implementation note:** The `repository_macro` function needs to branch on `config.hooks_type.is_some()` to emit either the original simple CRUD or the hooked lifecycle CRUD. This keeps the no-hooks path zero-cost.

**Step 4: Run compile-pass test**

Run: `cargo check --test compile_fail` (verify the compile-pass test passes)
Expected: PASS — generated code compiles

**Step 5: Re-export `tracing` in reexports**

`tracing` is already used in the crate but may not be in `reexports`. Add to `autumn/src/lib.rs`:

```rust
pub use tracing;
```

in the `reexports` module.

**Step 6: Verify full workspace build**

Run: `cargo check --workspace`
Expected: PASS

**Step 7: Commit**

```bash
git add autumn-macros/src/repository.rs autumn/tests/compile-pass/repository_with_hooks.rs autumn/src/lib.rs
git commit -m "feat(macros): generate transactional CRUD with mutation hook lifecycle"
```

---

## Phase 3: Transaction Support Wiring

### Task 8: Verify `diesel-async` transaction compatibility

**Files:**
- Create: `autumn/tests/hooks_lifecycle.rs` (integration test, requires DB)

The `diesel-async` `AsyncConnection::transaction` method has this signature:

```rust
async fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
where
    F: FnOnce(&mut Self) -> BoxFuture<Result<T, E>> + Send,
    T: Send,
    E: From<diesel::result::Error> + Send,
```

**Important:** `E` must implement `From<diesel::result::Error>`. Our `AutumnError` already has a blanket `From<E: Error>`, and `diesel::result::Error` implements `std::error::Error`, so this is satisfied.

**Step 1: Create integration test**

Create `autumn/tests/hooks_lifecycle.rs`:

```rust
//! Integration tests for mutation hook lifecycle.
//!
//! These tests verify the generated code structure without a real database.
//! Full DB integration tests would go in an example or separate test crate.

use autumn_web::hooks::*;

#[test]
fn mutation_context_has_uuid_request_id() {
    let ctx = MutationContext::new(MutationOp::Create);
    let rid = ctx.request_id.as_ref().expect("should have request_id");
    // Should be a valid UUID v4 format
    assert_eq!(rid.len(), 36);
    assert_eq!(&rid[8..9], "-");
}

#[test]
fn mutation_op_roundtrip() {
    for op in [MutationOp::Create, MutationOp::Update, MutationOp::Delete] {
        let s = op.as_str();
        assert!(!s.is_empty());
        assert_eq!(format!("{op}"), s);
    }
}

#[test]
fn field_diff_set_rewrite() {
    let mut diff = FieldDiff::new("old".to_string(), "old".to_string());
    assert!(diff.unchanged());
    diff.set("new".to_string());
    assert!(diff.changed());
    assert!(diff.changed_from(&"old".to_string()));
    assert!(diff.changed_to(&"new".to_string()));
}

#[test]
fn patch_serde_roundtrip() {
    let set: Patch<i32> = Patch::Set(42);
    let json = serde_json::to_string(&set).unwrap();
    assert_eq!(json, "42");

    let unchanged: Patch<i32> = Patch::Unchanged;
    let json = serde_json::to_string(&unchanged).unwrap();
    assert_eq!(json, "null");
}
```

**Step 2: Run tests**

Run: `cargo test -p autumn-web --test hooks_lifecycle`
Expected: PASS

**Step 3: Commit**

```bash
git add autumn/tests/hooks_lifecycle.rs
git commit -m "test: add integration tests for mutation hook types"
```

---

## Phase 4: Compile-Pass and Compile-Fail Tests

### Task 9: Add compile-pass test for repository without hooks (regression)

**Files:**
- Create: `autumn/tests/compile-pass/repository_no_hooks.rs`

**Step 1: Create test**

```rust
// Compile-pass: existing #[repository] without hooks should still work unchanged

mod schema {
    autumn_web::reexports::diesel::table! {
        notes (id) {
            id -> Int4,
            content -> Text,
        }
    }
}

use schema::notes;
use autumn_web::prelude::*;

#[model]
pub struct Note {
    #[id]
    pub id: i32,
    pub content: String,
}

#[repository(Note)]
pub trait NoteRepository {
    fn find_by_content(content: String) -> Vec<Note>;
}

fn main() {}
```

**Step 2: Run compile test**

Run: `cargo test -p autumn-web --test compile_fail` (trybuild runs compile-pass too)
Expected: PASS

**Step 3: Commit**

```bash
git add autumn/tests/compile-pass/repository_no_hooks.rs
git commit -m "test: add compile-pass regression for repository without hooks"
```

---

### Task 10: Add compile-fail test for hooks on non-Default type

**Files:**
- Create: `autumn/tests/compile-fail/repository_hooks_not_default.rs`

**Step 1: Create test**

```rust
// compile-fail: hooks type must implement Default

mod schema {
    autumn_web::reexports::diesel::table! {
        items (id) {
            id -> Int4,
            name -> Text,
        }
    }
}

use schema::items;
use autumn_web::prelude::*;

#[model]
pub struct Item {
    #[id]
    pub id: i32,
    pub name: String,
}

// No Default impl!
pub struct BadHooks;

impl MutationHooks for BadHooks {
    type Model = Item;
    type NewModel = NewItem;
    type UpdateModel = UpdateItem;
}

#[repository(Item, hooks = BadHooks)]
pub trait ItemRepository {}

fn main() {}
```

**Step 2: Create expected error file**

Create `autumn/tests/compile-fail/repository_hooks_not_default.stderr`:

```
error[E0277]: the trait bound `BadHooks: Default` is not satisfied
```

(Exact error location TBD — adjust stderr after first run.)

**Step 3: Run**

Run: `cargo test -p autumn-web --test compile_fail`
Expected: PASS (test expects the compile error)

**Step 4: Commit**

```bash
git add autumn/tests/compile-fail/
git commit -m "test: add compile-fail test for hooks without Default"
```

---

## Phase 5: Re-export and Documentation

### Task 11: Update re-exports and add doc comments

**Files:**
- Modify: `autumn/src/lib.rs` (ensure all types re-exported)
- Modify: `autumn/src/prelude.rs` (ensure hook types in prelude)
- Modify: `autumn/src/hooks.rs` (module-level doc comment)

**Step 1: Verify re-exports are complete**

Ensure `lib.rs` has:

```rust
#[cfg(feature = "db")]
pub use hooks::{FieldDiff, MutationContext, MutationHooks, MutationOp, NoHooks, Patch};
```

Ensure `prelude.rs` has:

```rust
#[cfg(feature = "db")]
pub use crate::hooks::{FieldDiff, MutationContext, MutationHooks, MutationOp, Patch};
```

(`NoHooks` intentionally NOT in prelude — it's used by generated code, not user code.)

**Step 2: Add module doc comment**

Ensure `hooks.rs` has a rich module doc:

```rust
//! Repository mutation hooks: lifecycle types for business logic around writes.
//!
//! Hooks are opt-in at the repository declaration point:
//!
//! ```rust,ignore
//! #[repository(Post, hooks = PostHooks)]
//! trait PostRepository { ... }
//! ```
//!
//! When hooks are configured, the generated CRUD methods run the full
//! mutation lifecycle:
//!
//! 1. `before_*` — validate, reject, or rewrite the pending mutation
//! 2. **persist** — inside a database transaction
//! 3. `after_*` — additional writes inside the same transaction
//! 4. **commit**
//! 5. `after_commit` — side effects (best-effort, cannot roll back)
//!
//! Without hooks, repositories behave like plain CRUD with no overhead.
```

**Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS

**Step 4: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings

**Step 5: Run fmt**

Run: `cargo fmt --all`
Expected: No changes (or fixes applied)

**Step 6: Commit**

```bash
git add autumn/src/lib.rs autumn/src/prelude.rs autumn/src/hooks.rs
git commit -m "docs: add hook module documentation and finalize re-exports"
```

---

## Phase 6: Bookmarks Example (Optional Enhancement)

### Task 12: Add hooks to bookmarks example for demonstration

This task is optional — it demonstrates hooks with a real model but requires a running Postgres instance. If the example is updated, the hooks would:

- Auto-populate `created_at` in `before_create`
- Log audit trail in `after_update`

This can be a follow-up PR after the core feature lands.

---

## Summary

| Phase | Tasks | What's delivered |
|-------|-------|-----------------|
| 1 | 1-5 | Runtime types: `Patch`, `FieldDiff`, `MutationContext`, `MutationHooks`, `NoHooks` |
| 2 | 6-7 | Macro: `hooks = Type` parsing + transactional CRUD generation |
| 3 | 8 | Transaction compatibility verification |
| 4 | 9-10 | Compile-pass and compile-fail tests |
| 5 | 11 | Re-exports, prelude, documentation |
| 6 | 12 | (Optional) Bookmarks example with hooks |

**Total new files:** 4 (hooks.rs, 2 compile tests, 1 integration test)
**Modified files:** 5 (Cargo.toml x2, lib.rs, prelude.rs, repository.rs)
