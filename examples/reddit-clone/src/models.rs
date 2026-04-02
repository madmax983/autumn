use crate::schema::{comments, posts, subreddits, users, votes};

// ── User ───────────────────────────────────────────────────────
// Manual model — password_hash should never be auto-exposed via API.

#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable, serde::Serialize)]
#[diesel(table_name = users)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(skip)]
    pub password_hash: String,
    pub karma: i64,
    pub role: String,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Debug, diesel::Insertable, serde::Deserialize)]
#[diesel(table_name = users)]
pub struct NewUser {
    pub username: String,
    pub password_hash: String,
}

// ── Subreddit ──────────────────────────────────────────────────

#[autumn_web::model]
pub struct Subreddit {
    #[id]
    pub id: i64,
    #[indexed]
    #[validate(length(min = 2, max = 32))]
    pub name: String,
    #[indexed]
    pub slug: String,
    pub description: String,
    pub creator_id: i64,
    #[default]
    pub subscriber_count: i64,
    #[default]
    pub created_at: chrono::NaiveDateTime,
}

// ── Post ───────────────────────────────────────────────────────

#[autumn_web::model]
pub struct Post {
    #[id]
    pub id: i64,
    #[validate(length(min = 1, max = 300))]
    pub title: String,
    #[indexed]
    pub slug: String,
    pub body: String,
    pub url: Option<String>,
    #[indexed]
    pub author_id: i64,
    #[indexed]
    pub subreddit_id: i64,
    #[default]
    pub score: i64,
    #[default]
    pub hot_rank: f64,
    #[default]
    pub comment_count: i64,
    #[default]
    pub created_at: chrono::NaiveDateTime,
    #[default]
    pub updated_at: chrono::NaiveDateTime,
}

// ── Comment ────────────────────────────────────────────────────

#[autumn_web::model]
pub struct Comment {
    #[id]
    pub id: i64,
    #[validate(length(min = 1))]
    pub body: String,
    #[indexed]
    pub author_id: i64,
    #[indexed]
    pub post_id: i64,
    pub parent_id: Option<i64>,
    #[default]
    pub score: i64,
    #[default]
    pub created_at: chrono::NaiveDateTime,
}

// ── Vote ───────────────────────────────────────────────────────
// Manual model — complex constraints.

#[allow(dead_code)] // Used by generated API routes; not directly referenced in app code
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable, serde::Serialize)]
#[diesel(table_name = votes)]
pub struct Vote {
    pub id: i64,
    pub user_id: i64,
    pub post_id: Option<i64>,
    pub comment_id: Option<i64>,
    pub value: i16,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Debug, diesel::Insertable)]
#[diesel(table_name = votes)]
pub struct NewVote {
    pub user_id: i64,
    pub post_id: Option<i64>,
    pub comment_id: Option<i64>,
    pub value: i16,
}
