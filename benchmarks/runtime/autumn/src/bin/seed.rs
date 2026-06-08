//! Seed the benchmark database with 1 000 deterministic posts and one API token.
//!
//! Run with:
//!   cargo run --bin seed
//!
//! Requires DATABASE_URL to be set.

use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};

diesel::table! {
    posts (id) {
        id -> Int8,
        title -> Text,
        body -> Text,
        published -> Bool,
        author -> Text,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    api_tokens (id) {
        id -> Int8,
        token -> Text,
        principal -> Text,
        created_at -> Timestamptz,
    }
}

#[derive(Insertable)]
#[diesel(table_name = posts)]
struct SeedPost {
    title: String,
    body: String,
    published: bool,
    author: String,
}

#[derive(Insertable)]
#[diesel(table_name = api_tokens)]
struct SeedToken {
    token: String,
    principal: String,
}

#[tokio::main]
async fn main() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let mut conn = AsyncPgConnection::establish(&db_url)
        .await
        .expect("failed to connect");

    diesel::delete(posts::table)
        .execute(&mut conn)
        .await
        .expect("truncate posts");

    diesel::delete(api_tokens::table)
        .execute(&mut conn)
        .await
        .expect("truncate api_tokens");

    let authors = ["alice", "bob", "carol", "dave", "eve"];
    let body_suffix = "Lorem ipsum dolor sit amet. ".repeat(3);

    let seed_posts: Vec<SeedPost> = (1..=1000)
        .map(|n| SeedPost {
            title: format!("Post number {n}"),
            body: format!(
                "This is the body of post number {n}. It contains enough text to be realistic. {body_suffix}"
            ),
            published: n % 3 != 0,
            author: authors[n % 5].to_owned(),
        })
        .collect();

    diesel::insert_into(posts::table)
        .values(&seed_posts)
        .execute(&mut conn)
        .await
        .expect("insert seed posts");

    diesel::insert_into(api_tokens::table)
        .values(SeedToken {
            token: "benchmark-token-abc123".into(),
            principal: "benchmark-user".into(),
        })
        .execute(&mut conn)
        .await
        .expect("insert api token");

    println!("Seeded 1000 posts and 1 API token.");
}
