# Path Helpers

Autumn generates typed URL path helpers from your route declarations.
Instead of hand-rolling `format!("/posts/{}", post.id)` at every call site,
you write `paths::show_post(post.id)` and let the compiler tell you when a
rename breaks a redirect, an `href`, or an `hx-post` attribute.

## The problem with `format!` strings

```rust
// ❌ Silent runtime 404 if you rename /posts → /articles
Redirect::to(&format!("/posts/{}", post.id))
```

When you rename a route, nothing points at the dozens of `format!("/posts/…")`
call sites that now build dead links.  The first feedback is a 404 in
production — or worse, an htmx request silently hitting a different handler.

## Quick start

```rust
use autumn_web::prelude::*;
use autumn_web::extract::Path;

#[get("/posts/{id}")]
async fn show_post(_id: Path<i64>) -> Markup { /* … */ }

#[get("/posts")]
async fn list_posts() -> Markup { /* … */ }

// Generate the `paths` module — one line, same list as routes![].
autumn_web::paths![show_post, list_posts];

// Usage anywhere that can see `paths`:
use crate::paths;

let show_url  = paths::show_post(42i64);        // PathBuilder → "/posts/42"
let list_url  = paths::list_posts();            // PathBuilder → "/posts"
let query_url = paths::list_posts()
    .with_query("page", 2)
    .with_query("tag", "rust web");             // "/posts?page=2&tag=rust%20web"
```

## Generating the `paths` module

Call `autumn_web::paths![]` **once** at the level where you define your routes
— typically `src/main.rs` or a router file — with the same handler list you
pass to `routes![]`.

```rust
// src/main.rs
autumn_web::paths![index, list_posts, show_post, create_post];

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index, list_posts, show_post, create_post])
        .run()
        .await;
}
```

> **Why `autumn_web::paths![]` instead of just `paths![]`?**
>
> `paths![]` generates a `pub mod paths { … }` block in the same scope.
> Importing the macro by name (`use autumn_web::paths`) would create a
> naming conflict with the generated module.  Using the fully-qualified macro
> path avoids the conflict.  If you use `use autumn_web::prelude::*` (which
> includes the macro), call `paths![]` directly; the glob import does not
> create the same conflict.

## Path parameter types

Helper signatures inherit the type from the handler's `Path<T>` extractor.

| Handler signature | Generated helper |
|-------------------|-----------------|
| `async fn show(Path(id): Path<i64>)` | `pub fn show(id: i64) -> PathBuilder` |
| `async fn get_comment(Path((post, cmt)): Path<(i64, i64)>)` | `pub fn get_comment(post: i64, cmt: i64) -> PathBuilder` |
| `async fn by_slug(_slug: Path<String>)` | `pub fn by_slug(slug: impl Display) -> PathBuilder` |
| `async fn list()` (no Path param) | `pub fn list() -> PathBuilder` |

`String` path params are promoted to `impl Display` so callers can pass
`&str` without allocating.  Other concrete types (e.g. `i64`, `u32`, custom
newtypes) are kept exact so typos are caught at the call site.

## Overriding the helper name

When two handlers in different modules share a function name, or when the
call-site reads better with a different name, use the `#[name = "…"]`
attribute:

```rust
#[get("/posts/{id}")]
#[name = "post_url"]
async fn show(Path(id): Path<i64>) -> Markup { /* … */ }

// Generated helper name:
paths::post_url(42i64)   // "/posts/42"
```

## Query strings

`PathBuilder::with_query` appends percent-encoded key=value pairs:

```rust
let url = paths::list_posts()
    .with_query("page", 2)
    .with_query("per_page", 20)
    .to_string();
// "/posts?page=2&per_page=20"
```

- The first call inserts `?`; subsequent calls insert `&`.
- Both key and value are percent-encoded per RFC 3986 (unreserved characters
  like `-`, `_`, `.`, `~` are preserved; `&`, `=`, `+`, space, etc. are
  escaped).

## Redirects

`PathBuilder::into_redirect()` converts the path directly into an
`axum::response::Redirect`:

```rust
// Before — string dance:
return Redirect::to(&format!("/posts/{}", post.id));

// After — zero boilerplate:
return paths::show_post(post.id).into_redirect();
```

## Module-qualified handlers

If your handlers live in submodules, pass the qualified path to `paths![]`:

```rust
autumn_web::paths![posts::list, posts::show, users::profile];

// Generated module:
// pub mod paths {
//     pub use super::posts::__autumn_path_list as list;
//     pub use super::posts::__autumn_path_show as show;
//     pub use super::users::__autumn_path_profile as profile;
// }
```

To avoid name collisions across modules, pair the module structure with the
`#[name = "…"]` override:

```rust
// routes/posts.rs
#[get("/posts/{id}")]
#[name = "posts_show"]
async fn show(Path(id): Path<i64>) -> Markup { /* … */ }

// routes/users.rs
#[get("/users/{id}")]
#[name = "users_show"]
async fn show(Path(id): Path<i64>) -> Markup { /* … */ }

// main.rs
autumn_web::paths![posts::show, users::show];
// paths::posts_show(1)  →  "/posts/1"
// paths::users_show(2)  →  "/users/2"
```

## Working with `PathBuilder`

`PathBuilder` implements:

| Trait / method | Purpose |
|----------------|---------|
| `Display` | `format!("{builder}")` or `builder.to_string()` |
| `Deref<Target = str>` | `&*builder` / automatic `&str` coercion |
| `From<PathBuilder> for String` | `.into()` |
| `AsRef<str>` | pass to any `fn(impl AsRef<str>)` |
| `.with_query(key, value)` | append a query pair (returns `PathBuilder`) |
| `.into_redirect()` | convert to `axum::response::Redirect` |

## Migration from `format!` strings

Search for hand-rolled paths in your codebase:

```sh
git grep -nE 'format!\("/' src/
```

Replace each hit:

```rust
// Before
href=(format!("/posts/{}", post.id))
hx_post=(format!("/posts/{}/votes", post.id))
Redirect::to(&format!("/posts/{}", post.id))

// After (assuming paths![] is declared and `use crate::paths`)
href=(paths::show_post(post.id))
hx_post=(paths::vote_post(post.id))
paths::show_post(post.id).into_redirect()
```

### Before / after: `examples/wiki`

```rust
// Before — hand-rolled in seven places
Redirect::to(&format!("/pages/{}", slug))
a href=(format!("/pages/{}/edit", slug))

// After — typed helpers, compile error on rename
paths::show_page(&slug).into_redirect()
a href=(paths::edit_page(&slug))
```

## `#[repository]` REST helpers

When you enable the auto-generated REST API with
`#[repository(Model, api = "/api/items")]`, Autumn also emits five typed path
helpers — one per CRUD verb — alongside the route-info companions:

| Helper | Signature | Resolves to |
|--------|-----------|-------------|
| `__autumn_path_{prefix}_api_list()` | `() -> PathBuilder` | `/api/items` |
| `__autumn_path_{prefix}_api_get(id)` | `(i64) -> PathBuilder` | `/api/items/{id}` |
| `__autumn_path_{prefix}_api_create()` | `() -> PathBuilder` | `/api/items` |
| `__autumn_path_{prefix}_api_update(id)` | `(i64) -> PathBuilder` | `/api/items/{id}` |
| `__autumn_path_{prefix}_api_delete(id)` | `(i64) -> PathBuilder` | `/api/items/{id}` |

`{prefix}` is the snake_case model name derived from the struct name (e.g.
`Bookmark` → `bookmark`).

```rust
// repositories.rs
#[autumn_web::repository(Bookmark, api = "/api/bookmarks")]
pub trait BookmarkRepository { /* … */ }

// bookmarks.rs  — use in Maud templates
html! {
    button hx-delete=(crate::repositories::__autumn_path_bookmark_api_delete(b.id))
           hx-confirm="Delete?" { "Delete" }
}
```

The helpers live in the same module as the `#[repository]` attribute, so use
the full crate path or a `use` import when calling them from a different module.

## Verification

After migrating, confirm all `format!` strings are gone:

```sh
git grep -nE 'format!\("/' examples/
# should return zero hits
```

And verify `cargo check` catches stale call sites when a path changes:

```sh
# Rename /posts/{id} → /articles/{id} in the macro attribute, then:
cargo check
# error[E0425]: cannot find function `show_post` in module `paths`
```
