// ── v0.2 Feature: Valid<Json<T>> extractor ──────────────────────
//
// Valid<Json<NewBookmark>> auto-validates the request body:
//   - url must be a valid URL (#[validate(url)])
//   - title must be 1-200 chars (#[validate(length(min = 1, max = 200))])
//
// Invalid requests get a 422 response with field-level error details:
//   { "error": { "status": 422, "message": "Validation failed",
//     "details": { "url": ["..."], "title": ["..."] } } }

use autumn_web::extract::Path;
use autumn_web::prelude::*;

use crate::models::NewBookmark;
use crate::repositories::{BookmarkRepository, PgBookmarkRepository};

#[get("/api/bookmarks")]
pub async fn list_json(
    repo: PgBookmarkRepository,
) -> AutumnResult<Json<Vec<crate::models::Bookmark>>> {
    Ok(Json(repo.find_all().await?))
}

#[post("/api/bookmarks")]
pub async fn create_json(
    repo: PgBookmarkRepository,
    Valid(Json(new)): Valid<Json<NewBookmark>>,
) -> AutumnResult<Json<crate::models::Bookmark>> {
    // `new` is guaranteed valid — url format and title length already checked
    Ok(Json(repo.save(&new).await?))
}

#[delete("/api/bookmarks/{id}")]
pub async fn delete_json(Path(id): Path<i64>, repo: PgBookmarkRepository) -> AutumnResult<String> {
    repo.delete_by_id(id).await?;
    Ok(String::new())
}
