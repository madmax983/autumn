//! JSON API routes for the blog application.
//!
//! These endpoints provide a REST-style API alongside the HTML routes,
//! demonstrating that Autumn handlers can return either HTML or JSON.

use autumn_web::{AutumnResult, Db, Json, get, post};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{NewPost, Post};
use crate::schema::posts;

/// Return all published posts as a JSON array.
#[get("/api/posts")]
pub async fn list_json(mut db: Db) -> AutumnResult<Json<Vec<Post>>> {
    let published = Post::published(&mut db).await?;
    Ok(Json(published))
}

/// Create a new post from a JSON body, return the created post as JSON.
#[post("/api/posts")]
pub async fn create_json(mut db: Db, body: Json<NewPost>) -> AutumnResult<Json<Post>> {
    let new_post = body.0.validated()?;

    let created: Post = diesel::insert_into(posts::table)
        .values(&new_post)
        .returning(Post::as_returning())
        .get_result(&mut db)
        .await?;

    Ok(Json(created))
}
