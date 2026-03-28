mod schema {
    autumn_web::reexports::diesel::table! {
        posts (id) {
            id -> Int8,
            title -> Text,
            status -> Text,
        }
    }
}

use schema::posts;

#[autumn_web::model]
pub struct Post {
    #[id]
    pub id: i64,
    pub title: String,
    pub status: String,
}

fn main() {
    let _f = PostField::Title;
    let _g = PostField::Status;

    // Verify traits
    let a = PostField::Title;
    let b = PostField::Title;
    assert!(a == b);
    let _debug = format!("{a:?}");
    let _clone = a.clone();
    let _copy = a;
}
