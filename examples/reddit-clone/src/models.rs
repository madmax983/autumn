use crate::schema::{comments, posts, subreddits, users, votes};

// Manual model -- password_hash should never be auto-exposed via API.

#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable, serde::Serialize)]
#[diesel(table_name = users)]
/// Struct documentation.
pub struct User {
    /// Item documentation.
    pub id: i64,
    /// Item documentation.
    pub username: String,
    #[serde(skip)]
    /// Item documentation.
    pub password_hash: String,
    /// Item documentation.
    pub karma: i64,
    /// Item documentation.
    pub role: String,
    /// Item documentation.
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Debug, Clone, diesel::Insertable, serde::Deserialize)]
#[diesel(table_name = users)]
/// Struct documentation.
pub struct NewUser {
    /// Item documentation.
    pub username: String,
    /// Item documentation.
    pub password_hash: String,
}

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

// Manual model -- complex constraints.

#[allow(dead_code)] // Used by generated API routes; not directly referenced in app code
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable, serde::Serialize)]
#[diesel(table_name = votes)]
/// Struct documentation.
pub struct Vote {
    /// Item documentation.
    pub id: i64,
    /// Item documentation.
    pub user_id: i64,
    /// Item documentation.
    pub post_id: Option<i64>,
    /// Item documentation.
    pub comment_id: Option<i64>,
    /// Item documentation.
    pub value: i16,
    /// Item documentation.
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Debug, diesel::Insertable)]
#[diesel(table_name = votes)]
/// Struct documentation.
pub struct NewVote {
    /// Item documentation.
    pub user_id: i64,
    /// Item documentation.
    pub post_id: Option<i64>,
    /// Item documentation.
    pub comment_id: Option<i64>,
    /// Item documentation.
    pub value: i16,
}
