//! Integration tests for path helper generation (issue #499).
//!
//! Each route macro emits a `__autumn_path_{name}` companion function.
//! The `paths![]` macro collects these into a `pub mod paths { ... }` block.

// `paths` is intentionally NOT imported directly — calling `autumn_web::paths![]`
// generates `pub mod paths { ... }` in this scope, and importing the macro by
// name would conflict with the generated module (both land in the same namespace).
use autumn_web::extract::Path;
use autumn_web::{delete, get, post, put};

// ── Test handlers ─────────────────────────────────────────────────────

#[get("/posts")]
async fn list_posts() -> &'static str {
    "posts"
}

#[get("/posts/{id}")]
async fn show_post(_id: Path<i64>) -> &'static str {
    "post"
}

#[post("/posts")]
async fn create_post() -> &'static str {
    "created"
}

#[put("/posts/{id}")]
async fn update_post(_id: Path<i64>) -> &'static str {
    "updated"
}

#[delete("/posts/{id}")]
async fn delete_post(_id: Path<i64>) -> &'static str {
    "deleted"
}

#[get("/posts/{post_id}/comments/{comment_id}")]
async fn show_comment(_ids: Path<(i64, i64)>) -> &'static str {
    "comment"
}

#[get("/about")]
async fn about_page() -> &'static str {
    "about"
}

#[get("/search/")]
async fn search_with_trailing_slash() -> &'static str {
    "search"
}

// Generate the `paths` module from the handlers above.
// Using the full qualified path avoids a name conflict between the imported
// `paths` macro and the `pub mod paths` this macro generates.
autumn_web::paths![
    list_posts,
    show_post,
    create_post,
    update_post,
    delete_post,
    show_comment,
    about_page,
    search_with_trailing_slash,
];

// ── PathBuilder structural tests ──────────────────────────────────────

#[test]
fn no_param_route_helper_returns_exact_path() {
    assert_eq!(paths::list_posts().to_string(), "/posts");
}

#[test]
fn no_param_route_helper_display_matches_to_string() {
    let builder = paths::list_posts();
    assert_eq!(format!("{builder}"), "/posts");
}

#[test]
fn post_route_no_param_returns_path() {
    assert_eq!(paths::create_post().to_string(), "/posts");
}

#[test]
fn trailing_slash_is_preserved() {
    assert_eq!(paths::search_with_trailing_slash().to_string(), "/search/");
}

#[test]
fn static_path_returns_exact_path() {
    assert_eq!(paths::about_page().to_string(), "/about");
}

// ── Single-param path helpers ─────────────────────────────────────────

#[test]
fn single_param_get_helper_formats_id() {
    assert_eq!(paths::show_post(42i64).to_string(), "/posts/42");
}

#[test]
fn single_param_put_helper_formats_id() {
    assert_eq!(paths::update_post(7i64).to_string(), "/posts/7");
}

#[test]
fn single_param_delete_helper_formats_id() {
    assert_eq!(paths::delete_post(99i64).to_string(), "/posts/99");
}

#[test]
fn single_param_helper_id_zero() {
    assert_eq!(paths::show_post(0i64).to_string(), "/posts/0");
}

#[test]
fn single_param_helper_negative_id() {
    assert_eq!(paths::show_post(-1i64).to_string(), "/posts/-1");
}

// ── Multi-param path helpers ──────────────────────────────────────────

#[test]
fn multi_param_helper_formats_both_params() {
    assert_eq!(
        paths::show_comment(1i64, 2i64).to_string(),
        "/posts/1/comments/2"
    );
}

#[test]
fn multi_param_helper_different_values() {
    assert_eq!(
        paths::show_comment(100i64, 999i64).to_string(),
        "/posts/100/comments/999"
    );
}

// ── PathBuilder query string helpers ─────────────────────────────────

#[test]
fn with_query_appends_question_mark_and_pair() {
    assert_eq!(
        paths::list_posts().with_query("page", 2).to_string(),
        "/posts?page=2"
    );
}

#[test]
fn with_query_chains_multiple_pairs_with_ampersand() {
    let path = paths::list_posts()
        .with_query("page", 2)
        .with_query("per_page", 10)
        .to_string();
    assert!(path.starts_with("/posts?"));
    assert!(path.contains("page=2"));
    assert!(path.contains("per_page=10"));
}

#[test]
fn with_query_percent_encodes_spaces() {
    assert_eq!(
        paths::list_posts()
            .with_query("q", "hello world")
            .to_string(),
        "/posts?q=hello%20world"
    );
}

#[test]
fn with_query_percent_encodes_ampersand_in_value() {
    let path = paths::list_posts()
        .with_query("tags", "rust&web")
        .to_string();
    assert!(path.contains("rust%26web"));
}

#[test]
fn with_query_on_path_with_param() {
    assert_eq!(
        paths::show_post(42i64)
            .with_query("format", "json")
            .to_string(),
        "/posts/42?format=json"
    );
}

// ── String conversion ─────────────────────────────────────────────────

#[test]
fn path_builder_into_string() {
    let s: String = paths::list_posts().into();
    assert_eq!(s, "/posts");
}

#[test]
fn path_builder_as_str_via_deref() {
    let builder = paths::list_posts();
    assert_eq!(&*builder, "/posts");
}
