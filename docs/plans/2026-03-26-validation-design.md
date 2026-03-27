# Validation Design

**Date:** 2026-03-26
**Status:** Validated (post six-hats review)
**Target:** v0.2.0

## Overview

Derive-based validation for request types, built on the `validator` crate, with a `Validated<T>` newtype that provides compile-time proof that validation has run. Integrates with `#[model]`, `#[repository]`, and Autumn's extractor system.

**Key innovations:**
- `Validated<T>` newtype — the type system proves validation happened, not a runtime flag
- Repository methods accept `Validated<T>` — can't save unvalidated data
- `#[model]` is the single source of truth for validation rules — no duplication
- Auto-validation via `Valid<Json<T>>` extractor, manual via `.validate()?`

## User-Facing API

### Declaring validation rules on a model

```rust
#[model]
struct Post {
    #[id]
    id: i32,
    #[indexed]
    #[validate(length(min = 1, max = 200))]
    title: String,
    #[validate(length(min = 1))]
    body: String,
    #[validate(email)]
    author_email: String,
    published: bool,
}
```

The `#[model]` macro propagates `#[validate(...)]` attributes onto generated `NewPost` and `UpdatePost` types, and adds `#[derive(Validate)]` to both.

### Auto-validation in handlers (the 90% case)

```rust
#[post("/posts")]
async fn create(
    repo: PgPostRepository,
    Valid(Json(new)): Valid<Json<NewPost>>
) -> AutumnResult<Json<Post>> {
    // `new` is Validated<NewPost> — guaranteed valid
    Ok(Json(repo.save(&new).await?))
}
```

### Manual validation (when you need custom error handling)

```rust
#[post("/posts")]
async fn create_with_custom_errors(
    repo: PgPostRepository,
    Json(new): Json<NewPost>
) -> AutumnResult<Json<Post>> {
    let validated = new.validate()?;  // returns AutumnResult<Validated<NewPost>>
    Ok(Json(repo.save(&validated).await?))
}
```

### Repository integration

```rust
#[repository(Post)]
trait PostRepository {
    fn find_by_title(title: &str) -> Vec<Post>;
}

// Generated save/update signatures accept Validated<T>:
// async fn save(&self, new: &Validated<NewPost>) -> AutumnResult<Post>;
// async fn update(&self, id: i32, changes: &Validated<UpdatePost>) -> AutumnResult<Post>;
```

### Non-HTTP contexts (background jobs, CLI, tests)

```rust
let new_post = NewPost {
    title: "Hello".into(),
    body: "World".into(),
    author_email: "test@example.com".into(),
    published: false,
};
let validated = new_post.validate()?;  // Validated<NewPost>
repo.save(&validated).await?;
```

## Design Decisions

### Built on `validator` crate
Autumn re-exports `validator` and wraps it with integration glue. The `validator` crate provides 40+ built-in rules (length, email, url, range, regex, custom, nested, etc.). Autumn's value-add is the integration, not reimplementing string checks.

### `Validated<T>` newtype

```rust
/// Proof that `T` has passed validation. Cannot be constructed without validation.
pub struct Validated<T>(T);

impl<T> Validated<T> {
    /// Only way to construct: call validate() on a Validate impl.
    /// Not public — only created by the validation system.
    pub(crate) fn new(value: T) -> Self {
        Self(value)
    }

    /// Unwrap the validated value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Validated<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> AsRef<T> for Validated<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}
```

Key properties:
- `Validated::new()` is `pub(crate)` — user code can't construct it without validation
- `Deref<Target = T>` — reading fields works transparently
- No `DerefMut` — can't mutate validated data into an invalid state
- `into_inner()` gives ownership back when you need the raw type

### Validation trait extension

```rust
/// Extension trait that adds `.validate()` to any type implementing validator::Validate
pub trait ValidateExt: validator::Validate + Sized {
    fn validate(self) -> AutumnResult<Validated<Self>> {
        validator::Validate::validate(&self)?;
        Ok(Validated::new(self))
    }
}

// Blanket impl for all types that implement validator::Validate
impl<T: validator::Validate> ValidateExt for T {}
```

### `Valid<T>` extractor

```rust
/// Extractor that deserializes and validates in one step.
/// Wraps any inner extractor (Json, Form, Query).
pub struct Valid<T>(pub T);
```

When used as `Valid<Json<NewPost>>`:
1. Axum deserializes the JSON body into `NewPost`
2. `validator::Validate::validate()` runs on the `NewPost`
3. If valid: handler receives `Validated<NewPost>` via destructuring
4. If invalid: returns 422 with structured error response

Implementation sketch:
```rust
#[axum::async_trait]
impl<S, T, Inner> FromRequest<S> for Valid<Inner>
where
    S: Send + Sync,
    Inner: FromRequest<S>,
    Inner::Rejection: Into<AutumnError>,
    // T is the inner type that implements Validate
{
    type Rejection = AutumnError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let inner = Inner::from_request(req, state).await.map_err(Into::into)?;
        // extract the T from the inner, validate, wrap in Validated
        // exact mechanism depends on inner extractor type
    }
}
```

### `#[model]` propagation — verbatim attribute pass-through

**Critical implementation detail:** `#[model]` must pass `#[validate(...)]` attributes through *verbatim* to generated types. It must NOT interpret or parse them — that's `#[derive(Validate)]`'s job in a later compilation pass. This avoids coupling between the two macros and prevents attribute conflicts with Diesel derives.

When `#[model]` encounters `#[validate(...)]` on a field:

1. The `Post` (query) type: `#[validate]` attributes are **stripped** (no validation needed for DB reads)
2. The `NewPost` (insert) type: `#[validate]` attributes are **propagated verbatim**, `#[derive(Validate)]` added
3. The `UpdatePost` (changeset) type: `#[validate]` attributes are **propagated verbatim** onto each `Option<T>` field. Validation only runs on `Some` values (handled by `validator` crate natively).

```rust
// Generated NewPost — validate required fields
#[derive(Insertable, Serialize, Deserialize, Validate)]
#[diesel(table_name = posts)]
pub struct NewPost {
    #[validate(length(min = 1, max = 200))]
    pub title: String,
    #[validate(length(min = 1))]
    pub body: String,
    #[validate(email)]
    pub author_email: String,
    pub published: bool,
}

// Generated UpdatePost — validate optional fields when present
#[derive(AsChangeset, Serialize, Deserialize, Validate)]
#[diesel(table_name = posts)]
pub struct UpdatePost {
    #[validate(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[validate(length(min = 1))]
    pub body: Option<String>,
    #[validate(email)]
    pub author_email: Option<String>,
    pub published: Option<bool>,
}
```

Note: `validator` already handles `Option<T>` — it skips validation when the value is `None` and validates the inner value when `Some`.

### Unified error response for deserialization + validation

Deserialization failures (bad JSON → 400) and validation failures (invalid fields → 422) must use the **same** `AutumnError` JSON shape. Axum's default `JsonRejection` is wrapped into `AutumnError` so API consumers see a consistent format for all "your request body is wrong" errors:

```json
// Deserialization error (400)
{
    "error": {
        "status": 400,
        "message": "Invalid request body",
        "details": {
            "_body": ["expected `,` or `}` at line 3 column 1"]
        }
    }
}

// Validation error (422)
{
    "error": {
        "status": 422,
        "message": "Validation failed",
        "details": {
            "title": ["must be between 1 and 200 characters"],
            "author_email": ["must be a valid email"]
        }
    }
}
```

Deserialization errors use `"_body"` as the key since they're not field-specific. The `Valid<T>` extractor handles both cases and always returns `AutumnError`.

### `UpdatePost` cross-field validation limitations

Cross-field validators like `#[validate(must_match(other = "password_confirm"))]` may not behave as expected on `UpdatePost` where all fields are `Option<T>`. When one field is `Some` and the referenced field is `None`, the behavior depends on `validator`'s handling.

**Recommendation:** For cross-field validation on partial updates, use `#[validate(custom(function = "..."))]` which can inspect all fields and handle the `Option` semantics explicitly. Document this limitation clearly.

## Error Response Format

Consistent with existing `AutumnError` JSON shape:

```json
{
    "error": {
        "status": 422,
        "message": "Validation failed",
        "details": {
            "title": ["must be between 1 and 200 characters"],
            "author_email": ["must be a valid email address"]
        }
    }
}
```

Implementation:
```rust
impl From<validator::ValidationErrors> for AutumnError {
    fn from(errors: validator::ValidationErrors) -> Self {
        let details: HashMap<String, Vec<String>> = errors
            .field_errors()
            .into_iter()
            .map(|(field, errs)| {
                let messages = errs.iter()
                    .map(|e| e.message.clone().unwrap_or_default().to_string())
                    .collect();
                (field.to_string(), messages)
            })
            .collect();

        AutumnError::validation(details)
    }
}
```

The `AutumnError` type gets a new `validation()` constructor and a `details` field:
```rust
impl AutumnError {
    pub fn validation(details: HashMap<String, Vec<String>>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: "Validation failed".into(),
            details: Some(details),
        }
    }
}
```

## Available Validation Rules

Re-exported from `validator`, available via `#[validate(...)]`:

| Rule | Example | Description |
|---|---|---|
| `length(min, max)` | `#[validate(length(min = 1, max = 200))]` | String/collection length |
| `email` | `#[validate(email)]` | Email format |
| `url` | `#[validate(url)]` | URL format |
| `range(min, max)` | `#[validate(range(min = 0, max = 100))]` | Numeric range |
| `contains(pattern)` | `#[validate(contains(pattern = "@"))]` | Substring check |
| `does_not_contain` | `#[validate(does_not_contain(pattern = "spam"))]` | Inverse substring |
| `must_match(field)` | `#[validate(must_match(other = "password_confirm"))]` | Field equality |
| `regex(path)` | `#[validate(regex(path = "RE_SLUG"))]` | Regex match |
| `custom(function)` | `#[validate(custom(function = "validate_slug"))]` | Custom function |
| `nested` | `#[validate(nested)]` | Validate nested structs |
| `required` | `#[validate(required)]` | Option must be Some |

### Custom validation functions

```rust
fn validate_slug(slug: &str) -> Result<(), validator::ValidationError> {
    if slug.chars().all(|c| c.is_alphanumeric() || c == '-') {
        Ok(())
    } else {
        Err(validator::ValidationError::new("invalid_slug"))
    }
}

#[model]
struct Post {
    #[id]
    id: i32,
    #[validate(custom(function = "validate_slug"))]
    slug: String,
}
```

## Full Example — Everything Together

```rust
use autumn_web::{get, post, put, delete, routes, Json, extract::Path, Valid, AutumnResult, AutumnError};

// --- Model with validation + indexing ---
#[autumn_web::model]
struct Post {
    #[id]
    id: i32,
    #[indexed]
    #[validate(length(min = 1, max = 200))]
    title: String,
    #[validate(length(min = 1))]
    body: String,
    #[validate(email)]
    author_email: String,
    #[indexed]
    published: bool,
}

// --- Repository ---
#[autumn_web::repository(Post)]
trait PostRepository {
    fn find_by_published(published: bool) -> Vec<Post>;
    fn find_by_title(title: &str) -> Option<Post>;
}

// --- Handlers ---
#[get("/posts")]
async fn list(repo: PgPostRepository) -> AutumnResult<Json<Vec<Post>>> {
    Ok(Json(repo.find_by_published(true).await?))
}

#[post("/posts")]
async fn create(
    repo: PgPostRepository,
    Valid(Json(new)): Valid<Json<NewPost>>
) -> AutumnResult<Json<Post>> {
    // `new` is Validated<NewPost> — title length, body length, email all checked
    Ok(Json(repo.save(&new).await?))
}

#[put("/posts/{id}")]
async fn update(
    repo: PgPostRepository,
    id: Path<i32>,
    Valid(Json(changes)): Valid<Json<UpdatePost>>
) -> AutumnResult<Json<Post>> {
    // Only provided fields are validated (Option::None skipped)
    Ok(Json(repo.update(*id, &changes).await?))
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![list, create, update])
        .run()
        .await;
}
```

## Spring Boot Comparison

| Spring Boot | Autumn |
|---|---|
| `@Valid @RequestBody NewPost` | `Valid(Json(new)): Valid<Json<NewPost>>` |
| `@NotBlank`, `@Size`, `@Email` | `#[validate(length(...))]`, `#[validate(email)]` |
| `MethodArgumentNotValidException` | `AutumnError` with 422 + details map |
| Runtime validation | Runtime validation with compile-time proof |
| Can pass unvalidated to service layer | `Validated<T>` prevents this at compile time |
| `BindingResult` for manual error handling | `.validate()?` returns `AutumnResult<Validated<T>>` |
| Annotations on entity AND DTO | `#[validate]` on `#[model]`, auto-propagated to generated types |
| Custom validator class | `#[validate(custom(function = "..."))]` |

## Implementation Order

1. **Add `validator` dependency** — workspace dep, re-export from `autumn_web`
2. **`Validated<T>` newtype** — core type with `Deref`, no `DerefMut`, `pub(crate)` constructor
3. **`ValidateExt` trait** — blanket `.validate() -> AutumnResult<Validated<T>>` for all `Validate` impls
4. **`AutumnError` extension** — `validation()` constructor, `details` field, `From<ValidationErrors>`
5. **`Valid<T>` extractor** — wraps Json/Form/Query, auto-validates, returns `Validated<T>`
6. **`#[model]` enhancement** — propagate `#[validate]` attrs to `NewPost`/`UpdatePost`, add `#[derive(Validate)]`
7. **`#[repository]` integration** — `save()` and `update()` accept `&Validated<T>`
8. **Tests** — unit tests for `Validated<T>`, extractor tests, error format tests, trybuild compile-fail tests
9. **Example** — update todo-app/blog to use validation

## Risks & Mitigations (from Six Hats Review)

| Risk | Mitigation |
|---|---|
| Deserialization (400) and validation (422) return different JSON shapes | Wrap Axum's `JsonRejection` into `AutumnError` — unified format with `details` map |
| `#[validate]` attributes conflict with Diesel derive macros | `#[model]` passes attributes verbatim — never interprets them, lets `#[derive(Validate)]` handle them in a later pass |
| `UpdatePost` cross-field validators (`must_match`) behave unexpectedly with `Option<T>` fields | Document limitation, recommend `custom(function)` for cross-field partial update validation |
| `Validated::new()` is `pub(crate)` — internal misuse possible | Limit construction to `ValidateExt::validate()` and `Valid<T>` extractor only. Add internal doc comments warning against direct construction. |
| IDE confusion from `Valid<Json<NewPost>>` yielding `Validated<NewPost>` | Document the type transformation clearly. IDE type hints will show the correct destructured type. |

## Dependencies

### Integration with `#[repository]` design
- `save()` signature: `async fn save(&self, new: &Validated<NewPost>) -> AutumnResult<Post>`
- `update()` signature: `async fn update(&self, id: i32, changes: &Validated<UpdatePost>) -> AutumnResult<Post>`
- Other CRUD methods (find, delete, count) don't involve validation

### Integration with `#[model]` design
- `#[validate(...)]` attributes are parsed by `#[model]` and forwarded to generated types
- `Post` (query type): attributes stripped
- `NewPost` (insert type): attributes propagated, `Validate` derived
- `UpdatePost` (changeset type): attributes propagated, `Validate` derived, `Option<T>` handled by validator
