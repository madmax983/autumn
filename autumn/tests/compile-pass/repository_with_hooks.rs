mod schema {
    autumn_web::reexports::diesel::table! {
        articles (id) {
            id -> Int8,
            title -> Text,
            body -> Text,
        }
    }
}

use schema::articles;
use autumn_web::prelude::*;

#[autumn_web::model]
pub struct Article {
    #[id]
    pub id: i64,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Default)]
pub struct ArticleHooks;

impl MutationHooks for ArticleHooks {
    type Model = Article;
    type NewModel = NewArticle;
    type UpdateModel = UpdateArticle;
}

#[autumn_web::repository(Article, hooks = ArticleHooks)]
pub trait ArticleRepository {
    fn find_by_title(title: String) -> Vec<Article>;
}

#[autumn_web::repository(Article, hooks = ArticleHooks, commit_hooks = true, no_upsert_trait)]
pub trait ArticleCommitRepository {}

fn main() {}
