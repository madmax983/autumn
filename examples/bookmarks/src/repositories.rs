// ── v0.2 Feature: #[repository] macro ───────────────────────────
//
// Generates PgBookmarkRepository with:
//   - 7 auto-generated CRUD methods (find_by_id, find_all, save, etc.)
//   - Derived queries parsed from method names below
//   - FromRequestParts extractor (use as handler parameter)
//
// The trait stays alive for mockability in tests.
//
// Types that must be in scope: Bookmark, NewBookmark, UpdateBookmark,
// and the diesel schema module (bookmarks).

use crate::models::{Bookmark, NewBookmark, UpdateBookmark};
use crate::schema::bookmarks;

#[autumn_web::repository(Bookmark)]
pub trait BookmarkRepository {
    // Derived query: SELECT * FROM bookmarks WHERE tag = $1
    fn find_by_tag(tag: String) -> Vec<Bookmark>;

    // Derived query: SELECT * FROM bookmarks WHERE alive = $1
    fn find_by_alive(alive: bool) -> Vec<Bookmark>;
}
