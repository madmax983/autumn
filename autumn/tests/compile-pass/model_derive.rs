use autumn_web::model;

// Diesel schema definition needed for the macro to work
diesel::table! {
    users (id) {
        id -> Integer,
        name -> Text,
    }
}

#[model(table = "users")]
pub struct User {
    pub id: i32,
    pub name: String,
}

// Test inferred table name
diesel::table! {
    blog_posts (id) {
        id -> Integer,
        title -> Text,
    }
}

#[model]
pub struct BlogPost {
    pub id: i32,
    pub title: String,
}

fn main() {}
