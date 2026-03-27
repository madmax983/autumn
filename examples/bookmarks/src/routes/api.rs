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

use crate::models::{Bookmark, NewBookmark};

#[get("/api/bookmarks")]
pub async fn list_json(mut db: Db) -> AutumnResult<Json<Vec<Bookmark>>> {
    Ok(Json(Bookmark::all(&mut db).await?))
}

#[post("/api/bookmarks")]
pub async fn create_json(
    mut db: Db,
    Valid(Json(new)): Valid<Json<NewBookmark>>,
) -> AutumnResult<Json<Bookmark>> {
    // `new` is guaranteed valid — url format and title length already checked
    Ok(Json(Bookmark::create(&new, &mut db).await?))
}

#[delete("/api/bookmarks/{id}")]
pub async fn delete_json(id: Path<i32>, mut db: Db) -> AutumnResult<String> {
    Bookmark::delete(*id, &mut db).await?;
    Ok(String::new())
}
