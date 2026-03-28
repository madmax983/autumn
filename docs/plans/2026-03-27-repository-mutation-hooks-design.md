# Repository Mutation Hooks Design

**Date:** 2026-03-27
**Status:** Validated
**Target:** Follow-on to `#[repository]`

## Overview

Autumn should support repository-scoped mutation hooks for business logic around
record writes. This is the framework-level equivalent of "triggers," but kept
explicit, typed, and bounded to generated repositories instead of becoming a
global automation swamp.

The feature exists to cover application concerns such as:

- derived field updates
- audit trails
- denormalized counters
- approval and status transition rules
- tenant enforcement
- durable CDC and outbox emission

Transactions and hooks solve different problems. Transactions define atomicity.
Mutation hooks define when business logic runs relative to that atomicity.

## Goals

- Make business logic around `create`, `update`, and `delete` first-class
- Keep write orchestration inside generated repositories
- Provide ergonomic "did this change?" helpers for update logic
- Support reliable CDC via transactional outbox writes
- Keep the common case boring when an app does not opt into hooks

## Non-Goals

- Global app-wide trigger registries
- Dynamic runtime rule systems
- Hook execution for raw `Db` writes
- Multiple stacked hook chains per repository
- Recursive repository-triggered mutations

## User-Facing API

Hooks are opt-in at the repository declaration point:

```rust
#[repository(Post, hooks = PostHooks)]
trait PostRepository {
    fn find_by_status(status: Status) -> Vec<Post>;
}
```

The application defines one hook owner for the repository:

```rust
pub struct PostHooks;

impl MutationHooks<Post, NewPost, UpdatePost> for PostHooks {
    async fn before_update(
        ctx: &mut MutationContext<Post>,
        current: &Post,
        draft: &mut UpdateDraft<Post>,
    ) -> AutumnResult<()> {
        if draft.status().changed_to(&Status::Approved) {
            draft.approved_at().set(Some(ctx.now));
        }

        if draft.title().changed() && draft.slug().unchanged() {
            draft.slug().set(Some(slugify(draft.title().after())));
        }

        Ok(())
    }
}
```

If no hook type is configured, the repository uses a generated `NoHooks`
implementation and behaves like plain CRUD.

## Lifecycle

Generated repository write methods own the full mutation lifecycle:

1. Begin transaction
2. Load current record for `update` and `delete`
3. Build mutation context and merged draft state
4. Run `before_create`, `before_update`, or `before_delete`
5. Persist the mutation
6. Run `after_create`, `after_update`, or `after_delete` inside the transaction
7. Commit transaction
8. Run `after_commit`

Semantics:

- `before_*` hooks may validate, reject, or rewrite the pending mutation
- `after_*` hooks may perform additional transactional writes
- `after_commit` may perform side effects, but it cannot affect commit outcome

## Core Types

### `Patch<T>`

Sparse update input must support tri-state semantics for nullable fields:

```rust
pub enum Patch<T> {
    Unchanged,
    Set(T),
    Clear,
}
```

`Clear` maps to SQL `NULL` for nullable columns. For non-nullable columns,
`Clear` is rejected during validation before the query is issued.

### `UpdateDraft<T>`

Repositories merge the patch into a concrete proposed record before invoking
business logic:

```rust
pub struct UpdateDraft<T> {
    before: T,
    after: T,
    changed_fields: SmallVec<[T::Field; 8]>,
}
```

Hooks reason over the final proposed state instead of sparse patch plumbing.

### Generated Diff Helpers

The `#[model]` macro should generate:

- a field enum such as `PostField`
- per-field draft accessors such as `draft.status()`
- object-level helpers such as `draft.did_change(PostField::Status)`

Each field helper exposes:

- `changed()`
- `unchanged()`
- `before()`
- `after()`
- `changed_from(&T)`
- `changed_to(&T)`
- `was_set()`
- `was_cleared()`
- `set(...)`

This removes the need for endless `old.value != new.value` checks in hook code.

## Context

`MutationContext<T>` should carry the data hook authors actually need:

- actor identity
- request metadata
- correlation/request id
- current timestamp
- transaction handle
- model metadata

This gives repository-scoped hooks enough context for audit and policy logic
without pushing framework users down into raw request plumbing.

## Reliability and CDC

`after_commit` is useful for CDC orchestration, but it is not the durable source
of truth by itself. Reliable CDC should use a transactional outbox pattern:

- `after_*` writes an outbox row inside the same transaction as the mutation
- `after_commit` nudges a dispatcher, queue, or local notifier

This avoids the failure mode where the commit succeeds but a direct publish
fails.

Error semantics are strict:

- errors in `before_*` abort the mutation
- errors in `after_*` abort the mutation
- errors in `after_commit` do not roll back the mutation

Generated event payloads should include:

- repository or model name
- operation type
- primary key
- actor and request metadata
- changed fields
- optional before snapshot
- optional after snapshot
- occurrence timestamp
- stable event id or idempotency key

## Guard Rails

- Raw `Db` writes bypass hooks by design
- Only one hook owner exists per repository
- Hooks cannot recursively call the same repository mutation API
- Repository ordering is deterministic because there is only one hook owner

These constraints deliberately trade flexibility for debuggability.

## Testing Strategy

The feature should be tested at three levels:

1. Unit tests for `UpdateDraft<T>` diff helpers and merge semantics
2. Integration tests for repository mutation lifecycle and rollback behavior
3. Integration tests for outbox row creation and `after_commit` failure behavior

Minimum cases:

- `before_update` can rewrite derived fields
- `before_*` rejection rolls back the mutation
- `after_*` rejection rolls back the mutation
- `after_commit` failure does not roll back committed data
- nullable field updates correctly distinguish unchanged, set, and clear
- changed-field helpers match the final persisted diff

## Recommendation

Implement repository-scoped mutation hooks as an explicit opt-in extension of
the repository macro. Keep the transaction boundary inside generated CRUD,
present hooks with merged-draft semantics, and use transactional outbox writes
for durable CDC.
