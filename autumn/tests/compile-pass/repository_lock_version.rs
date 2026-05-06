// Compile-pass: #[repository] on a model with #[lock_version]
// The generated update() should use optimistic concurrency control.

mod schema {
    autumn_web::reexports::diesel::table! {
        articles (id) {
            id -> Int8,
            title -> Text,
            lock_version -> Int4,
        }
    }
}

use schema::articles;

#[autumn_web::model]
pub struct Article {
    #[id]
    pub id: i64,
    pub title: String,
    #[lock_version]
    pub lock_version: i32,
}

#[autumn_web::repository(Article)]
pub trait ArticleRepository {}

fn assert_update_signature_unchanged() {
    // The update() method keeps its original signature — callers pass the
    // UpdateArticle which now carries lock_version as a required field.
    fn _check<R: ArticleRepository>(repo: &R) {
        let update = UpdateArticle {
            title: autumn_web::hooks::Patch::Set("new".to_string()),
            lock_version: 0,
        };
        let _: std::pin::Pin<Box<dyn std::future::Future<Output = autumn_web::AutumnResult<Article>>>> =
            Box::pin(repo.update(1_i64, &update));
    }
}

fn main() {
    assert_update_signature_unchanged();
}
