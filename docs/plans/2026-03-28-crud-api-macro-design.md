# CRUD API Macro Design

**Date**: 2026-03-28
**Status**: Accepted
**Branch**: TBD

## Summary

Add `api = "/path"` parameter to the existing `#[repository]` macro to auto-generate 5 REST API endpoint handlers for any model. Zero new crates, zero runtime components — just additional `quote!` codegen in the repository macro.

## Motivation

Every Autumn REST API follows the same pattern: extract repository, call CRUD method, wrap in `Json`. The bookmarks example has 37 lines of hand-written API routes that are pure boilerplate. This macro eliminates that entirely.

Comparable to Django REST Framework's `ModelViewSet` — define the model and get a full API.

## Developer Experience

```rust
use crate::models::{Bookmark, NewBookmark, UpdateBookmark};
use crate::schema::bookmarks;

#[autumn_web::repository(Bookmark, hooks = BookmarkHooks, api = "/api/v1/bookmarks")]
pub trait BookmarkRepository {
    fn find_by_tag(tag: String) -> Vec<Bookmark>;
}
```

This generates the existing repository struct (`PgBookmarkRepository`) plus 5 handler functions:

| Function | Method | Path | Body | Returns |
|----------|--------|------|------|---------|
| `bookmark_api_list` | `GET` | `/api/v1/bookmarks` | — | `Json<Vec<Bookmark>>` |
| `bookmark_api_get` | `GET` | `/api/v1/bookmarks/{id}` | — | `Json<Bookmark>` |
| `bookmark_api_create` | `POST` | `/api/v1/bookmarks` | `Valid<Json<NewBookmark>>` | `201 Created` + `Json<Bookmark>` |
| `bookmark_api_update` | `PUT` | `/api/v1/bookmarks/{id}` | `Json<UpdateBookmark>` | `Json<Bookmark>` |
| `bookmark_api_delete` | `DELETE` | `/api/v1/bookmarks/{id}` | — | `204 No Content` |

### Naming Convention

`{snake_case_model}_api_{operation}` — e.g., `Bookmark` produces `bookmark_api_list`, `Page` produces `page_api_list`.

### Route Registration

Handlers register via `routes![]` like any other route:

```rust
autumn_web::app()
    .routes(routes![
        routes::bookmarks::list,
        routes::bookmarks::create,
        // Auto-generated REST API
        repositories::bookmark_api_list,
        repositories::bookmark_api_get,
        repositories::bookmark_api_create,
        repositories::bookmark_api_update,
        repositories::bookmark_api_delete,
    ])
    .run()
    .await;
```

### Versioning

Path prefix in the string: `api = "/api/v1/bookmarks"`. No framework magic — when you need v2, create a new model/repository with different hooks, validation, or fields. Each version is a fully independent code path with its own Rust types.

```rust
#[autumn_web::repository(Bookmark, api = "/api/v1/bookmarks")]
pub trait BookmarkRepository {}

#[autumn_web::repository(BookmarkV2, api = "/api/v2/bookmarks")]
pub trait BookmarkV2Repository {}
```

## Generated Code

The macro emits these 5 functions (shown with fully-qualified paths as they appear in expansion):

```rust
#[get("/api/v1/bookmarks")]
pub async fn bookmark_api_list(
    repo: PgBookmarkRepository,
) -> AutumnResult<Json<Vec<Bookmark>>> {
    Ok(Json(repo.find_all().await?))
}

#[get("/api/v1/bookmarks/{id}")]
pub async fn bookmark_api_get(
    ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
    repo: PgBookmarkRepository,
) -> AutumnResult<Json<Bookmark>> {
    let record = repo.find_by_id(id).await?
        .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg("not found"))?;
    Ok(Json(record))
}

#[post("/api/v1/bookmarks")]
pub async fn bookmark_api_create(
    repo: PgBookmarkRepository,
    ::autumn_web::prelude::Valid(Json(new)): ::autumn_web::prelude::Valid<Json<NewBookmark>>,
) -> AutumnResult<(::http::StatusCode, Json<Bookmark>)> {
    let record = repo.save(&new).await?;
    Ok((::http::StatusCode::CREATED, Json(record)))
}

#[put("/api/v1/bookmarks/{id}")]
pub async fn bookmark_api_update(
    ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
    repo: PgBookmarkRepository,
    Json(patch): Json<UpdateBookmark>,
) -> AutumnResult<Json<Bookmark>> {
    let record = repo.update(id, &patch).await?;
    Ok(Json(record))
}

#[delete("/api/v1/bookmarks/{id}")]
pub async fn bookmark_api_delete(
    ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
    repo: PgBookmarkRepository,
) -> AutumnResult<::http::StatusCode> {
    repo.delete_by_id(id).await?;
    Ok(::http::StatusCode::NO_CONTENT)
}
```

### Key Details

- **Create uses `Valid<Json<T>>`** — `#[validate]` attrs on `NewModel` are enforced automatically. 422 on failure.
- **Get returns 404** when `find_by_id` returns `None`.
- **Hooks fire automatically** — `save()` and `update()` already run the hook lifecycle.
- **Fully-qualified imports** — no name collisions in user code.

## Implementation

### Scope

All changes in `autumn-macros/src/repository.rs`:

1. **Parse `api` parameter** — add `api_path: Option<String>` to `RepoConfig`, parse `api = "/path"` alongside `hooks` and `table`.
2. **Generate handler functions** — when `api_path` is `Some`, emit 5 `quote!` blocks with the handler functions.
3. **Derive function names** — `model_name.to_snake_case()` + `_api_` + operation.

Estimated: ~80-100 lines of codegen added to the existing file.

### No New Dependencies

The macro emits `#[get]`/`#[post]`/etc. annotated functions — the same ones a user writes by hand. Route macros already handle Axum wiring. `Json`, `Path`, `Valid`, and `AutumnResult` are already exported.

## Testing

### 1. Compile-pass: repository_with_api.rs

Proves the macro expands without errors and all 5 handler functions exist:

```rust
#[autumn_web::model]
pub struct Widget {
    #[id] pub id: i64,
    pub name: String,
}

#[autumn_web::repository(Widget, api = "/api/widgets")]
pub trait WidgetRepository {}

fn main() {
    let _ = widget_api_list;
    let _ = widget_api_get;
    let _ = widget_api_create;
    let _ = widget_api_update;
    let _ = widget_api_delete;
}
```

### 2. Compile-pass: repository_with_hooks_and_api.rs

Proves hooks + api compose:

```rust
#[autumn_web::repository(Widget, hooks = WidgetHooks, api = "/api/v1/widgets")]
pub trait WidgetRepository {}
```

### 3. Bookmarks example migration

Convert the existing `routes/api.rs` to use the macro. Delete the file, add `api = "/api/bookmarks"` to the repository, verify the app serves the same JSON responses.

## Migration: Bookmarks Example

**Before** (3 files):
- `routes/api.rs` — 37 lines of hand-written handlers
- `routes/mod.rs` — `pub mod api;`
- `main.rs` — 3 manual route registrations

**After** (1 line added):
- `repositories.rs` — add `api = "/api/bookmarks"`
- `main.rs` — replace 3 manual routes with 5 generated ones
- Delete `routes/api.rs`

Net: **-37 lines**, **+2 endpoints** (get-by-id, update).

## Future Considerations (Not In Scope)

- **Pagination** — `?page=1&per_page=20` on list endpoint
- **Filtering** — `?tag=rust&status=active` query params
- **OpenAPI spec generation** — auto-generate swagger docs from the macro
- **Opt-out** — `api(exclude(delete))` to skip specific operations
- **Nested resources** — `/api/pages/{page_id}/revisions`

These are natural extensions but not needed for v1.
