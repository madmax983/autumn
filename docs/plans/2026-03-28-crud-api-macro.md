# CRUD API Macro Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `api = "/path"` to `#[repository]` that auto-generates 5 REST API handler functions (list, get, create, update, delete).

**Architecture:** Extend `parse_repo_args` in `autumn-macros/src/repository.rs` to accept an `api_path` string. When present, emit 5 additional `#[get]`/`#[post]`/`#[put]`/`#[delete]` annotated handler functions alongside the existing repository struct and trait. No new crates or runtime components.

**Tech Stack:** `syn`, `quote`, `proc_macro2` (existing macro crate deps). Generated code uses `Json`, `Path`, `Valid`, `AutumnResult`, `AutumnError::not_found_msg` — all already exported by autumn-web.

---

### Task 1: Parse `api` Parameter

**Files:**
- Modify: `autumn-macros/src/repository.rs:18-62` (RepoConfig + parse_repo_args)

**Step 1: Write the failing test**

Add to the existing `mod tests` block in `autumn-macros/src/repository.rs` (after line 662):

```rust
#[test]
fn parse_repo_args_with_api() {
    let tokens: proc_macro2::TokenStream =
        r#"Post, api = "/api/posts""#.parse().unwrap();
    let config = parse_repo_args(tokens).unwrap();
    assert_eq!(config.model_name.to_string(), "Post");
    assert_eq!(config.api_path.as_deref(), Some("/api/posts"));
}

#[test]
fn parse_repo_args_with_hooks_and_api() {
    let tokens: proc_macro2::TokenStream =
        r#"Post, hooks = PostHooks, api = "/api/v1/posts""#.parse().unwrap();
    let config = parse_repo_args(tokens).unwrap();
    assert_eq!(config.model_name.to_string(), "Post");
    assert!(config.hooks_type.is_some());
    assert_eq!(config.api_path.as_deref(), Some("/api/v1/posts"));
}

#[test]
fn parse_repo_args_without_api() {
    let tokens: proc_macro2::TokenStream = "Post".parse().unwrap();
    let config = parse_repo_args(tokens).unwrap();
    assert!(config.api_path.is_none());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p autumn-macros -- parse_repo_args_with_api`
Expected: FAIL — `api_path` field doesn't exist on `RepoConfig`

**Step 3: Implement parsing**

Add `api_path: Option<String>` to `RepoConfig` (line 18-22):

```rust
struct RepoConfig {
    model_name: Ident,
    table_name: String,
    hooks_type: Option<Ident>,
    api_path: Option<String>,
}
```

Add the `api` parser branch in `parse_repo_args` (inside `syn::meta::parser`, after the `table` branch at line 38):

```rust
} else if meta.path.is_ident("api") {
    let value: LitStr = meta.value()?.parse()?;
    api_path = Some(value.value());
    Ok(())
}
```

Update the error message at line 44:

```rust
Err(meta.error("expected model name, table = \"...\", hooks = Type, or api = \"/path\""))
```

Initialize `api_path` at top of function (line 27) and include in the return struct:

```rust
let mut api_path: Option<String> = None;
// ...
Ok(RepoConfig {
    model_name: model,
    table_name: table,
    hooks_type,
    api_path,
})
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p autumn-macros`
Expected: ALL pass (including 3 new + all existing)

**Step 5: Commit**

```bash
git add autumn-macros/src/repository.rs
git commit -m "feat(macros): parse api = \"/path\" in #[repository] attribute"
```

---

### Task 2: Generate API Handler Functions

**Files:**
- Modify: `autumn-macros/src/repository.rs:168-575` (repository_macro function)

**Step 1: Write the compile-pass test**

Create `autumn/tests/compile-pass/repository_with_api.rs`:

```rust
mod schema {
    autumn_web::reexports::diesel::table! {
        widgets (id) {
            id -> Int8,
            name -> Text,
        }
    }
}

use schema::widgets;
use autumn_web::prelude::*;

#[autumn_web::model]
pub struct Widget {
    #[id]
    pub id: i64,
    pub name: String,
}

#[autumn_web::repository(Widget, api = "/api/widgets")]
pub trait WidgetRepository {}

fn main() {
    // Verify all 5 handler functions were generated
    let _ = widget_api_list;
    let _ = widget_api_get;
    let _ = widget_api_create;
    let _ = widget_api_update;
    let _ = widget_api_delete;
}
```

Register it in `autumn/tests/compile_fail.rs` (the compile_pass_tests function, after repository_with_hooks):

```rust
#[cfg(feature = "db")]
t.pass("tests/compile-pass/repository_with_api.rs");
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p autumn-web --test compile_fail -- compile_pass`
Expected: FAIL — `widget_api_list` not found

**Step 3: Implement codegen**

In `repository_macro()` in `autumn-macros/src/repository.rs`, after the final `quote! { ... }` block that generates the trait, struct, impl, and extractor (around line 479), add API handler generation.

Add this code just before the final `quote!` block's closing brace (before line 575). Build it as a separate `let api_handlers = ...` and include it in the final output.

First, derive the function name prefix from the model name. Add a helper at the top of the file (after the imports, around line 16):

```rust
fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}
```

Then in `repository_macro()`, after the existing struct/trait/impl generation but still inside the function, add:

```rust
let api_handlers = if let Some(ref api_path) = config.api_path {
    let prefix = to_snake_case(&model_name.to_string());
    let list_fn = format_ident!("{prefix}_api_list");
    let get_fn = format_ident!("{prefix}_api_get");
    let create_fn = format_ident!("{prefix}_api_create");
    let update_fn = format_ident!("{prefix}_api_update");
    let delete_fn = format_ident!("{prefix}_api_delete");

    let list_path = api_path.clone();
    let get_path = format!("{api_path}/{{id}}");
    let create_path = api_path.clone();
    let update_path = format!("{api_path}/{{id}}");
    let delete_path = format!("{api_path}/{{id}}");

    quote! {
        #[::autumn_web::reexports::axum::routing::get(#list_path)]
        #vis async fn #list_fn(
            repo: #pg_name,
        ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<Vec<#model_name>>> {
            Ok(::autumn_web::prelude::Json(repo.find_all().await?))
        }

        #[::autumn_web::reexports::axum::routing::get(#get_path)]
        #vis async fn #get_fn(
            ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
            repo: #pg_name,
        ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<#model_name>> {
            let record = repo.find_by_id(id).await?
                .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg("not found"))?;
            Ok(::autumn_web::prelude::Json(record))
        }

        #[::autumn_web::reexports::axum::routing::post(#create_path)]
        #vis async fn #create_fn(
            repo: #pg_name,
            ::autumn_web::prelude::Valid(::autumn_web::prelude::Json(new)): ::autumn_web::prelude::Valid<::autumn_web::prelude::Json<#new_name>>,
        ) -> ::autumn_web::AutumnResult<(::autumn_web::reexports::http::StatusCode, ::autumn_web::prelude::Json<#model_name>)> {
            let record = repo.save(&new).await?;
            Ok((::autumn_web::reexports::http::StatusCode::CREATED, ::autumn_web::prelude::Json(record)))
        }

        #[::autumn_web::reexports::axum::routing::put(#update_path)]
        #vis async fn #update_fn(
            ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
            repo: #pg_name,
            ::autumn_web::prelude::Json(patch): ::autumn_web::prelude::Json<#update_name>,
        ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<#model_name>> {
            let record = repo.update(id, &patch).await?;
            Ok(::autumn_web::prelude::Json(record))
        }

        #[::autumn_web::reexports::axum::routing::delete(#delete_path)]
        #vis async fn #delete_fn(
            ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
            repo: #pg_name,
        ) -> ::autumn_web::AutumnResult<::autumn_web::reexports::http::StatusCode> {
            repo.delete_by_id(id).await?;
            Ok(::autumn_web::reexports::http::StatusCode::NO_CONTENT)
        }
    }
} else {
    quote! {}
};
```

**IMPORTANT**: The route annotations above use the Axum attribute-style macros (`#[get(...)]`). But autumn uses its OWN route macros that generate `__autumn_route_info_*` companion functions. The generated handlers must use `#[autumn_web::get(...)]`, `#[autumn_web::post(...)]`, etc. — NOT Axum's native routing macros.

So the actual annotations must be:

```rust
#[autumn_web::get(#list_path)]
// NOT #[::autumn_web::reexports::axum::routing::get(#list_path)]
```

However, since we're inside a proc macro, we can't invoke another proc macro via `quote!`. The autumn route macros (`#[get]`, `#[post]`, etc.) generate a companion `__autumn_route_info_{fn_name}()` function that returns `(method, path)`. We need to generate this companion function directly instead of relying on the route macro.

Check what the route macro generates by reading `autumn-macros/src/route.rs` to understand the `__autumn_route_info_*` pattern. Then generate the companion functions inline.

Read `autumn-macros/src/route.rs` to find the exact pattern, then emit both the handler function AND its `__autumn_route_info_{name}` companion. The companion typically looks like:

```rust
#[doc(hidden)]
pub fn __autumn_route_info_widget_api_list() -> (&'static str, &'static str) {
    ("GET", "/api/widgets")
}
```

Include `api_handlers` in the final output by appending it to the existing `quote!` block.

**Step 4: Run test to verify it passes**

Run: `cargo test -p autumn-web --test compile_fail -- compile_pass`
Expected: PASS — all compile-pass tests green

**Step 5: Run full test suite**

Run: `cargo test -p autumn-web -p autumn-macros`
Expected: ALL pass

**Step 6: Commit**

```bash
git add autumn-macros/src/repository.rs autumn/tests/compile-pass/repository_with_api.rs autumn/tests/compile_fail.rs
git commit -m "feat(macros): generate CRUD API handlers from api = \"/path\""
```

---

### Task 3: Compile-pass Test for hooks + api Combined

**Files:**
- Create: `autumn/tests/compile-pass/repository_with_hooks_and_api.rs`
- Modify: `autumn/tests/compile_fail.rs`

**Step 1: Create the test**

Create `autumn/tests/compile-pass/repository_with_hooks_and_api.rs`:

```rust
mod schema {
    autumn_web::reexports::diesel::table! {
        gadgets (id) {
            id -> Int8,
            name -> Text,
            status -> Text,
        }
    }
}

use schema::gadgets;
use autumn_web::prelude::*;

#[autumn_web::model]
pub struct Gadget {
    #[id]
    pub id: i64,
    pub name: String,
    pub status: String,
}

#[derive(Clone, Default)]
pub struct GadgetHooks;

impl MutationHooks for GadgetHooks {
    type Model = Gadget;
    type NewModel = NewGadget;
    type UpdateModel = UpdateGadget;
}

#[autumn_web::repository(Gadget, hooks = GadgetHooks, api = "/api/v1/gadgets")]
pub trait GadgetRepository {
    fn find_by_status(status: String) -> Vec<Gadget>;
}

fn main() {
    // Verify all 5 API handlers exist
    let _ = gadget_api_list;
    let _ = gadget_api_get;
    let _ = gadget_api_create;
    let _ = gadget_api_update;
    let _ = gadget_api_delete;
}
```

Register in `autumn/tests/compile_fail.rs` (compile_pass_tests, after repository_with_api):

```rust
#[cfg(feature = "db")]
t.pass("tests/compile-pass/repository_with_hooks_and_api.rs");
```

**Step 2: Run test**

Run: `cargo test -p autumn-web --test compile_fail -- compile_pass`
Expected: PASS

**Step 3: Commit**

```bash
git add autumn/tests/compile-pass/repository_with_hooks_and_api.rs autumn/tests/compile_fail.rs
git commit -m "test: add compile-pass for repository with hooks + api combined"
```

---

### Task 4: Migrate Bookmarks Example

**Files:**
- Modify: `examples/bookmarks/src/repositories.rs` (add `api = "/api/bookmarks"`)
- Delete: `examples/bookmarks/src/routes/api.rs`
- Modify: `examples/bookmarks/src/routes/mod.rs` (remove `pub mod api;`)
- Modify: `examples/bookmarks/src/main.rs` (swap route registrations)

**Step 1: Add api to repository**

Change `examples/bookmarks/src/repositories.rs` from:

```rust
#[autumn_web::repository(Bookmark)]
pub trait BookmarkRepository {
```

to:

```rust
#[autumn_web::repository(Bookmark, api = "/api/bookmarks")]
pub trait BookmarkRepository {
```

**Step 2: Delete hand-written API routes**

Delete `examples/bookmarks/src/routes/api.rs`.

Remove `pub mod api;` from `examples/bookmarks/src/routes/mod.rs`.

**Step 3: Update main.rs route registrations**

In `examples/bookmarks/src/main.rs`, replace:

```rust
routes::api::list_json,
routes::api::create_json,
routes::api::delete_json,
```

with:

```rust
repositories::bookmark_api_list,
repositories::bookmark_api_get,
repositories::bookmark_api_create,
repositories::bookmark_api_update,
repositories::bookmark_api_delete,
```

**Step 4: Verify**

Run: `cargo check -p bookmarks`
Expected: compiles clean

Run: `cargo clippy -p bookmarks --all-targets`
Expected: no warnings

**Step 5: Commit**

```bash
git add examples/bookmarks/
git rm examples/bookmarks/src/routes/api.rs
git commit -m "refactor(bookmarks): replace hand-written API routes with api macro"
```

---

### Task 5: Add API to Wiki Example

**Files:**
- Modify: `examples/wiki/src/repositories.rs` (add `api = "/api/v1/pages"`)
- Modify: `examples/wiki/src/main.rs` (add API route registrations)

**Step 1: Add api to repository**

Change `examples/wiki/src/repositories.rs` from:

```rust
#[autumn_web::repository(Page, hooks = PageHooks)]
pub trait PageRepository {
```

to:

```rust
#[autumn_web::repository(Page, hooks = PageHooks, api = "/api/v1/pages")]
pub trait PageRepository {
```

**Step 2: Add API routes to main.rs**

In `examples/wiki/src/main.rs`, add to the `routes![]` block:

```rust
// Auto-generated REST API
repositories::page_api_list,
repositories::page_api_get,
repositories::page_api_create,
repositories::page_api_update,
repositories::page_api_delete,
```

**Step 3: Verify**

Run: `cargo check -p wiki`
Expected: compiles clean

Run: `cargo clippy -p wiki --all-targets`
Expected: no warnings

**Step 4: Commit**

```bash
git add examples/wiki/
git commit -m "feat(wiki): add REST API via api macro"
```

---

### Task 6: Final Verification

**Step 1: Full workspace check**

Run: `cargo check --workspace`
Expected: clean

**Step 2: Full test suite**

Run: `cargo test -p autumn-web --lib --test hooks_lifecycle --test compile_fail`
Run: `cargo test -p autumn-macros`
Run: `cargo test -p wiki`
Run: `cargo test -p bookmarks`
Expected: ALL pass

**Step 3: Clippy**

Run: `cargo clippy --workspace --all-targets`
Expected: no warnings from our code

**Step 4: Format**

Run: `cargo fmt --all --check`
Expected: clean
