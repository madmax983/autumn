//! Benchmark route handlers.
//!
//! Implements all five benchmark paths:
//!   1. JSON CRUD   – GET/POST/PATCH/DELETE /api/posts
//!   2. HTML page   – GET /posts (server-rendered)
//!   3. Validation  – POST /api/posts with invalid payload returns 422
//!   4. Auth guard  – GET /api/posts/protected requires Bearer token
//!   5. Detail      – GET /api/posts/:id

use autumn_web::extract::Path;
use autumn_web::prelude::{IntoResponse, Json, StatusCode};
use autumn_web::reexports::http::HeaderMap;
use autumn_web::{AutumnError, AutumnResult, Db, Markup, delete, get, html, patch, post};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::Serialize;

use crate::models::{ApiToken, NewPost, Post, PostUpdate};
use crate::schema::posts;

// ── Shared error response shape ───────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct ErrBody {
    error: String,
}

fn err_json(msg: impl Into<String>) -> (StatusCode, Json<ErrBody>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(ErrBody { error: msg.into() }),
    )
}

// ── Validation helpers ────────────────────────────────────────────────────────

#[cfg(test)]
pub fn validate_new_post_pub(p: &NewPost) -> Result<(), String> {
    validate_new_post(p)
}

fn validate_new_post(p: &NewPost) -> Result<(), String> {
    if p.title.trim().is_empty() {
        return Err("title must not be blank".into());
    }
    if p.title.len() > 255 {
        return Err("title must be 255 characters or fewer".into());
    }
    if p.body.trim().is_empty() {
        return Err("body must not be blank".into());
    }
    if p.author.trim().is_empty() {
        return Err("author must not be blank".into());
    }
    Ok(())
}

// ── JSON API ──────────────────────────────────────────────────────────────────

/// List the 50 most-recent posts as JSON.
#[get("/api/posts")]
pub async fn api_list(mut db: Db) -> AutumnResult<Json<Vec<Post>>> {
    Ok(Json(Post::all(&mut db).await?))
}

/// Get a single post by ID.
#[get("/api/posts/{id}")]
pub async fn api_show(id: Path<i64>, mut db: Db) -> AutumnResult<Json<Post>> {
    Ok(Json(Post::find(*id, &mut db).await?))
}

/// Create a post. Returns 422 on validation failure (benchmark validation path).
#[post("/api/posts")]
pub async fn api_create(
    mut db: Db,
    Json(body): Json<NewPost>,
) -> Result<(StatusCode, Json<Post>), (StatusCode, Json<ErrBody>)> {
    if let Err(msg) = validate_new_post(&body) {
        return Err(err_json(msg));
    }
    let post = diesel::insert_into(posts::table)
        .values(&body)
        .returning(Post::as_returning())
        .get_result(&mut *db)
        .await
        .map_err(|e| err_json(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(post)))
}

/// Partially update a post.
#[patch("/api/posts/{id}")]
pub async fn api_update(
    id: Path<i64>,
    mut db: Db,
    Json(body): Json<PostUpdate>,
) -> AutumnResult<Json<Post>> {
    let post = diesel::update(posts::table.find(*id))
        .set(&body)
        .returning(Post::as_returning())
        .get_result(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;
    Ok(Json(post))
}

/// Delete a post.
#[delete("/api/posts/{id}")]
pub async fn api_delete(id: Path<i64>, mut db: Db) -> AutumnResult<StatusCode> {
    let n = diesel::delete(posts::table.find(*id))
        .execute(&mut *db)
        .await?;
    if n == 0 {
        return Err(AutumnError::not_found_msg(format!(
            "post {} not found",
            *id
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── Auth-protected route ──────────────────────────────────────────────────────

/// Returns summary stats; requires a valid Bearer token.
///
/// The load test exercises this with the well-known seed token so every
/// framework can be measured on the same path.
#[get("/api/posts/protected")]
pub async fn api_protected(
    mut db: Db,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrBody>)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrBody {
                    error: "missing or invalid Authorization header".into(),
                }),
            )
        })?;

    let principal = ApiToken::verify(token, &mut db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrBody {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrBody {
                    error: "invalid token".into(),
                }),
            )
        })?;

    #[derive(Serialize)]
    struct Stats {
        principal: String,
        total_posts: i64,
    }

    let total: i64 = posts::table
        .count()
        .get_result(&mut *db)
        .await
        .map_err(|e| err_json(e.to_string()))?;

    Ok(Json(Stats {
        principal,
        total_posts: total,
    }))
}

// ── Server-rendered HTML ──────────────────────────────────────────────────────

fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                title { (title) }
            }
            body {
                (content)
            }
        }
    }
}

/// Server-rendered list of the 50 most-recent posts.
#[get("/posts")]
pub async fn html_list(mut db: Db) -> AutumnResult<Markup> {
    let all = Post::all(&mut db).await?;
    Ok(layout(
        "Posts",
        html! {
            h1 { "Posts" }
            ul {
                @for p in &all {
                    li {
                        a href=(format!("/posts/{}", p.id)) { (p.title) }
                        " — "
                        span { (p.author) }
                        @if !p.published { " [draft]" }
                    }
                }
            }
        },
    ))
}

/// Server-rendered single post detail.
#[get("/posts/{id}")]
pub async fn html_show(id: Path<i64>, mut db: Db) -> AutumnResult<Markup> {
    let p = Post::find(*id, &mut db).await?;
    Ok(layout(
        &p.title,
        html! {
            h1 { (p.title) }
            p { "By " (p.author) }
            @if !p.published { p { em { "Draft" } } }
            div { (p.body) }
        },
    ))
}
