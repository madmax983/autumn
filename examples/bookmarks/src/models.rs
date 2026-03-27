use crate::schema::bookmarks;

#[autumn_web::model]
pub struct Bookmark {
    #[id]
    pub id: i32,
    #[indexed]
    #[validate(url)]
    pub url: String,
    #[validate(length(min = 1, max = 200))]
    pub title: String,
    #[indexed]
    pub tag: String,
    #[default]
    pub alive: bool,
    #[default]
    pub created_at: chrono::NaiveDateTime,
}
