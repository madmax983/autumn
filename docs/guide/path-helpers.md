# Typed Path Helpers

Every route macro (`#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]`)
automatically emits a companion path-helper function alongside the handler:

```rust
#[get("/posts/{id}")]
pub async fn show(Path(id): Path<i64>, …) -> AutumnResult<Markup> { … }

// Generated automatically:
// pub fn __autumn_path_show(id: impl Display) -> String {
//     format!("/posts/{id}")
// }
```

## The `paths![]` macro

Use `autumn_web::paths![…]` at the bottom of a routes module to re-export
the helpers under clean names inside a `pub mod paths`:

```rust
autumn_web::paths![index, show, create, update, delete_post];

// Expands to:
// pub mod paths {
//     pub use super::__autumn_path_index as index;
//     pub use super::__autumn_path_show as show;
//     pub use super::__autumn_path_create as create;
//     pub use super::__autumn_path_update as update;
//     pub use super::__autumn_path_delete_post as delete_post;
// }
```

Templates and redirect calls then read:

```rust
// Before:
a href=(format!("/posts/{}", p.id)) { (p.title) }
Ok(Redirect::to(&format!("/posts/{}", post.id)))

// After:
a href=(paths::show(p.id)) { (p.title) }
Ok(Redirect::to(&paths::show(post.id)))
```

## Cross-module references

For links that point to a different module's routes, reference the
`__autumn_path_*` function directly:

```rust
// In subreddits.rs, linking to a post (defined in posts.rs):
a href=(super::posts::__autumn_path_show(&sub.slug, post_slug)) { … }
```

## Custom names with `name = "…"`

Override the helper's short name when the handler name would be ambiguous:

```rust
#[delete("/posts/{id}", name = "delete_post")]
pub async fn delete(…) { … }

// Generates __autumn_path_delete_post(id) instead of __autumn_path_delete
```

## Query strings with `PathExt`

The `PathExt` trait (re-exported from the prelude) adds `.with_query()` for
building query strings with RFC 3986 percent-encoding:

```rust
use autumn_web::PathExt;

let url = paths::list().with_query("page", 2).with_query("q", "hello world");
// → "/posts?page=2&q=hello%20world"
```

## `autumn_web::Redirect`

`axum::response::Redirect` is re-exported as `autumn_web::Redirect` and
included in the prelude. Use it instead of hand-rolled meta-refresh HTML:

```rust
// Before (do not use):
fn redirect_to(url: &str) -> Markup {
    html! { meta http-equiv="refresh" content=(format!("0; url={url}")); }
}

// After:
Ok(Redirect::to(&paths::show(post.id)))
```

## Repository API path helpers

`#[repository(api = "/api/posts")]` emits path helpers for its generated
REST endpoints alongside the route handlers:

```rust
#[autumn_web::repository(Post, api = "/api/posts")]
pub trait PostRepository { … }

// Generates:
// pub fn __autumn_path_post_api_list() -> String
// pub fn __autumn_path_post_api_get(id: impl Display) -> String
// pub fn __autumn_path_post_api_create() -> String
// pub fn __autumn_path_post_api_update(id: impl Display) -> String
// pub fn __autumn_path_post_api_delete(id: impl Display) -> String
```

Use them in templates to avoid hard-coding the API prefix:

```rust
hx-delete=(crate::repositories::__autumn_path_post_api_delete(post.id))
```
