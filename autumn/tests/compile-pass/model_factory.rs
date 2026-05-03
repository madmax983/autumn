mod schema {
    autumn_web::reexports::diesel::table! {
        articles (id) {
            id -> Int8,
            title -> Text,
            slug -> Text,
            score -> Int4,
            published -> Bool,
            note -> Nullable<Text>,
        }
    }
}

use schema::articles;

#[autumn_web::model]
pub struct Article {
    #[id]
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub score: i32,
    pub published: bool,
    pub note: Option<String>,
}

fn main() {
    // Factory constructor on the model type
    let _factory = Article::factory();

    // build() with all defaults
    let new_article: NewArticle = Article::factory().build();
    assert_eq!(new_article.title, "");
    assert_eq!(new_article.score, 0);
    assert!(!new_article.published);
    assert_eq!(new_article.note, None);

    // Override one field; others keep defaults
    let new_article = Article::factory().title("Hello").build();
    assert_eq!(new_article.title, "Hello");
    assert_eq!(new_article.slug, "");

    // Override multiple fields
    let new_article = Article::factory()
        .title("My Post")
        .slug("my-post")
        .score(42)
        .published(true)
        .note(Some("a note".to_string()))
        .build();
    assert_eq!(new_article.title, "My Post");
    assert_eq!(new_article.slug, "my-post");
    assert_eq!(new_article.score, 42);
    assert!(new_article.published);
    assert_eq!(new_article.note, Some("a note".to_string()));

    // Factory type is Default
    let _f: ArticleFactory = Default::default();

    // build() returns NewArticle (Insertable)
    let _: NewArticle = ArticleFactory::default().build();
}
