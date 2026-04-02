//! Comment routes — create and display comments on posts.
//!
//! Demonstrates: #[secured] for write routes, Db extractor for
//! raw queries, CsrfToken, htmx integration for inline comment
//! loading.

use autumn_web::extract::Path;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::Comment;
use crate::schema::{comments, posts, subreddits, users};

use super::layout::redirect_to;

#[derive(serde::Deserialize)]
pub struct CommentForm {
    pub body: String,
}

/// Create a new comment on a post.
#[secured]
#[post("/r/{sub_slug}/posts/{post_slug}/comments")]
pub async fn create(
    Path((sub_slug, post_slug)): Path<(String, String)>,
    session: Session,
    mut db: Db,
    form: Form<CommentForm>,
) -> AutumnResult<Markup> {
    let user_id: i64 = session
        .get("user_id")
        .await
        .ok_or_else(|| AutumnError::unauthorized_msg("Login required"))?
        .parse()
        .map_err(|_| AutumnError::bad_request_msg("Invalid session"))?;

    let body = form.0.body.trim().to_string();
    if body.is_empty() {
        return Err(AutumnError::unprocessable_msg("Comment cannot be empty"));
    }

    // Find the post
    let post_id: i64 = posts::table
        .inner_join(subreddits::table.on(posts::subreddit_id.eq(subreddits::id)))
        .filter(subreddits::slug.eq(&sub_slug))
        .filter(posts::slug.eq(&post_slug))
        .select(posts::id)
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Post not found"))?;

    // Insert comment
    diesel::insert_into(comments::table)
        .values((
            comments::body.eq(&body),
            comments::author_id.eq(user_id),
            comments::post_id.eq(post_id),
            comments::score.eq(1_i64),
        ))
        .execute(&mut *db)
        .await?;

    // Update comment count on post
    diesel::update(posts::table.find(post_id))
        .set(posts::comment_count.eq(posts::comment_count + 1))
        .execute(&mut *db)
        .await?;

    Ok(redirect_to(&format!(
        "/r/{sub_slug}/posts/{post_slug}"
    )))
}

/// htmx endpoint: load comments for a post (for lazy loading).
#[get("/r/{sub_slug}/posts/{post_slug}/comments")]
pub async fn list_comments(
    Path((sub_slug, post_slug)): Path<(String, String)>,
    mut db: Db,
) -> AutumnResult<Markup> {
    let post_id: i64 = posts::table
        .inner_join(subreddits::table.on(posts::subreddit_id.eq(subreddits::id)))
        .filter(subreddits::slug.eq(&sub_slug))
        .filter(posts::slug.eq(&post_slug))
        .select(posts::id)
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Post not found"))?;

    let post_comments: Vec<(Comment, String)> = comments::table
        .filter(comments::post_id.eq(post_id))
        .filter(comments::parent_id.is_null())
        .inner_join(users::table.on(comments::author_id.eq(users::id)))
        .order(comments::score.desc())
        .select((Comment::as_select(), users::username))
        .load(&mut *db)
        .await?;

    Ok(html! {
        @for (comment, author) in &post_comments {
            div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4" {
                div class="flex items-center gap-2 text-xs text-gray-400 mb-2" {
                    a href=(format!("/u/{author}"))
                       class="font-medium text-gray-600 hover:underline" {
                        "u/" (author)
                    }
                    "\u{2022} " (comment.score) " points"
                }
                div class="text-sm text-gray-700" {
                    @for para in comment.body.split("\n\n") {
                        @if !para.trim().is_empty() {
                            p class="mb-1" { (para.trim()) }
                        }
                    }
                }
            }
        }
        @if post_comments.is_empty() {
            p class="text-gray-400 text-center py-4 text-sm" { "No comments yet." }
        }
    })
}
