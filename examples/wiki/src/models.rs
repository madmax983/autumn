use crate::schema::{api_credentials, pages, revisions};

/// A stored third-party API token, encrypted at rest (issue #805).
///
/// The `token` field is declared `#[encrypted]`, so it persists as an opaque
/// AES-256-GCM envelope on disk while staying a plain `String` in Rust:
/// `repo.find(id)?.token` is plaintext, and inserts take plaintext. Configure
/// the key once with `autumn credentials edit`:
///
/// ```toml
/// [active_record_encryption]
/// primary_key = "<64 hex chars from `openssl rand -hex 32`>"
/// ```
///
/// See `docs/guide/attribute-encryption.md` for the full workflow.
#[autumn_web::model]
pub struct ApiCredential {
    #[id]
    pub id: i64,
    pub label: String,
    #[encrypted]
    pub token: String,
    #[default]
    pub created_at: chrono::NaiveDateTime,
}

#[autumn_web::model]
#[searchable(language = "english")]
pub struct Page {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
    pub slug: String,
    #[searchable(weight = "B")]
    pub body: String,
    pub status: String,
    #[lock_version]
    pub lock_version: i32,
    #[default]
    pub created_at: chrono::NaiveDateTime,
    #[default]
    pub updated_at: chrono::NaiveDateTime,
}

// Revision is manual — write-only from hooks, read-only from routes
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable, serde::Serialize)]
#[diesel(table_name = revisions)]
pub struct Revision {
    pub id: i64,
    pub page_id: i64,
    pub op: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub changed_by: Option<String>,
    pub summary: Option<String>,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Debug, Clone, diesel::Insertable)]
#[diesel(table_name = revisions)]
pub struct NewRevision {
    pub page_id: i64,
    pub op: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub changed_by: Option<String>,
    pub summary: Option<String>,
}
