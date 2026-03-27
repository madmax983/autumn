// ── Models ──────────────────────────────────────────────────────
//
// Manually defined until #[model] macro stabilizes for downstream
// crate usage. Showcases #[validate] for the NewBookmark type.

use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::schema::bookmarks;

#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = bookmarks)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Bookmark {
    pub id: i32,
    pub url: String,
    pub title: String,
    pub tag: String,
    pub alive: bool,
    pub created_at: chrono::NaiveDateTime,
}

// ── v0.2 Feature: #[validate] on request types ─────────────────
//
// validator::Validate derive + #[validate] attributes define the
// rules. Autumn's Valid<Json<T>> extractor auto-runs validation
// and returns 422 with field-level errors on failure.

#[derive(Debug, Clone, Insertable, Deserialize, Validate)]
#[diesel(table_name = bookmarks)]
pub struct NewBookmark {
    #[validate(url)]
    pub url: String,
    #[validate(length(min = 1, max = 200))]
    pub title: String,
    pub tag: String,
}

#[derive(Debug, Clone, AsChangeset, Deserialize)]
#[diesel(table_name = bookmarks)]
pub struct UpdateBookmark {
    pub url: Option<String>,
    pub title: Option<String>,
    pub tag: Option<String>,
    pub alive: Option<bool>,
}
