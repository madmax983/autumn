// Compile-pass: #[lock_version] field attribute on a model

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

/// Model with a `#[lock_version]` field.
#[autumn_web::model]
pub struct Article {
    #[id]
    pub id: i64,
    pub title: String,
    #[lock_version]
    pub lock_version: i32,
}

fn assert_no_lock_version_in_new() {
    // NewArticle must NOT have a `lock_version` field — the DB supplies the
    // initial value (DEFAULT 0). The lock_version is framework-managed.
    let _new = NewArticle {
        title: "hello".to_string(),
    };
}

fn assert_lock_version_plain_in_update() {
    // UpdateArticle must have `lock_version: i32` (not Patch<i32>), because
    // the client always sends the version they read; the framework increments it.
    let _update = UpdateArticle {
        title: autumn_web::hooks::Patch::Set("new title".to_string()),
        lock_version: 3,
    };
}

fn assert_methods_exist(article: &Article, update: &UpdateArticle) {
    // Model must expose the current stored version.
    let _actual: Option<i64> = article.__autumn_lock_version_actual();

    // UpdateModel must expose the client-supplied expected version.
    let _expected: Option<i64> = update.__autumn_lock_version_expected();
}

fn main() {
    assert_no_lock_version_in_new();
    assert_lock_version_plain_in_update();

    let article = Article {
        id: 1,
        title: "hello".to_string(),
        lock_version: 0,
    };
    let update = UpdateArticle {
        title: autumn_web::hooks::Patch::Unchanged,
        lock_version: 0,
    };
    assert_methods_exist(&article, &update);
}
