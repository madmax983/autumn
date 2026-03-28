use crate::schema::{pages, revisions};

#[autumn_web::model]
pub struct Page {
    #[id]
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub body: String,
    pub status: String,
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
