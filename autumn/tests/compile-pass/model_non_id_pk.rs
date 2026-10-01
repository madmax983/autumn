//! #3033 compile-pass: a `#[model]` whose primary key is not `id`.
//!
//! Two spellings: a renamed `#[id]` field (`post_uuid`), and a
//! `#[diesel(column_name = …)]` rename on an `id` field (`articles`). The
//! upsert/`on_conflict` paths, the list filter/sort arms, and the
//! association preload loaders must all address the physical pk column,
//! so this pins that the whole expansion type-checks. The child also pins
//! the new standalone `parent_pk` on `#[belongs_to]` (no counter cache):
//! its loader filters on and selects `posts.post_uuid` instead of
//! `posts.id`.
use autumn_web::model;

diesel::table! {
    posts (post_uuid) {
        post_uuid -> BigInt,
        title -> Text,
    }
}

diesel::table! {
    comments (id) {
        id -> BigInt,
        body -> Text,
        post_id -> BigInt,
    }
}

diesel::table! {
    articles (article_uuid) {
        article_uuid -> BigInt,
        title -> Text,
    }
}

#[model]
#[has_many(Comment)]
pub struct Post {
    #[id]
    pub post_uuid: i64,
    pub title: String,
}

#[model]
#[belongs_to(Post, parent_pk = "post_uuid")]
pub struct Comment {
    #[id]
    pub id: i64,
    pub body: String,
    pub post_id: i64,
}

#[model]
pub struct Article {
    #[id]
    #[diesel(column_name = article_uuid)]
    pub id: i64,
    pub title: String,
}

fn main() {
    // The models build; the generated upsert helpers name the real pk
    // column. (Only compiled — no database round-trip here.)
    let post = Post {
        post_uuid: 7,
        title: "non-id pk".to_string(),
    };
    assert_eq!(post.post_uuid, 7);
    let article = Article {
        id: 9,
        title: "renamed column".to_string(),
    };
    assert_eq!(article.id, 9);
}
