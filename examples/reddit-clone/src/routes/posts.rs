//! Post routes — front page, submit, view, edit, delete.
//!
//! Demonstrates: CRUD with the Db extractor, `CsrfToken` in forms,
//! #[secured] for write operations, htmx for voting and deletion,
//! Maud templates with Tailwind CSS.

use autumn_web::extract::Path;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{Post, Subreddit, User};
use crate::schema::{comments, posts, subreddits, users};
use crate::slugify::slugify;

use super::layout::{layout, redirect_to, time_ago, vote_controls};

/// (`post_id`, title, `post_slug`, score, `comment_count`, author, `sub_name`, `sub_slug`, `created_at`)
type PostSummary = (
    i64,
    String,
    String,
    i64,
    i64,
    String,
    String,
    String,
    chrono::NaiveDateTime,
);

// ── Front page — hot posts across all subreddits ───────────────

#[get("/")]
pub async fn front_page(session: Session, csrf: CsrfToken, mut db: Db) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;

    let hot_posts: Vec<PostSummary> = posts::table
        .inner_join(users::table.on(posts::author_id.eq(users::id)))
        .inner_join(subreddits::table.on(posts::subreddit_id.eq(subreddits::id)))
        .order(posts::hot_rank.desc())
        .limit(50)
        .select((
            posts::id,
            posts::title,
            posts::slug,
            posts::score,
            posts::comment_count,
            users::username,
            subreddits::name,
            subreddits::slug,
            posts::created_at,
        ))
        .load(&mut *db)
        .await?;

    Ok(layout(
        "Front Page",
        current_user.as_deref(),
        Some(csrf.token()),
        html! {
            // Sort tabs
            div class="flex items-center gap-4 mb-4 text-sm" {
                span class="px-3 py-1.5 bg-orange-100 text-orange-700 rounded-full font-medium" {
                    "Hot"
                }
                a href="/?sort=new" class="text-gray-500 hover:text-orange-600 px-3 py-1.5" {
                    "New"
                }
            }

            // Post list
            div class="space-y-2" {
                @for (post_id, title, post_slug, score, comment_count, author, sub_name, sub_slug, created_at) in &hot_posts {
                    div class="bg-white rounded-lg shadow-sm border border-gray-200 \
                               hover:border-orange-300 transition-colors" {
                        div class="flex items-start gap-3 p-4" {
                            (vote_controls(*post_id, *score))
                            div class="flex-1 min-w-0" {
                                a href=(format!("/r/{sub_slug}/posts/{post_slug}"))
                                   class="text-lg font-medium text-gray-900 hover:text-orange-600 \
                                          line-clamp-2" {
                                    (title)
                                }
                                div class="text-xs text-gray-400 mt-1" {
                                    a href=(format!("/r/{sub_slug}"))
                                       class="font-medium text-gray-600 hover:underline" {
                                        "r/" (sub_name)
                                    }
                                    " \u{2022} posted by "
                                    a href=(format!("/u/{author}"))
                                       class="text-gray-500 hover:underline" { "u/" (author) }
                                    " " (time_ago(created_at))
                                    " \u{2022} "
                                    a href=(format!("/r/{sub_slug}/posts/{post_slug}"))
                                       class="text-gray-500 hover:text-orange-600" {
                                        (comment_count) " comments"
                                    }
                                }
                            }
                        }
                    }
                }
                @if hot_posts.is_empty() {
                    div class="text-center py-16" {
                        p class="text-gray-400 text-lg mb-4" { "Nothing here yet!" }
                        p class="text-gray-400 text-sm" {
                            "Be the first to "
                            a href="/r" class="text-orange-600 hover:underline" {
                                "join a community"
                            }
                            " and post something."
                        }
                    }
                }
            }
        },
    ))
}

// ── Submit form (global — pick subreddit) ──────────────────────

#[secured]
#[get("/submit")]
pub async fn submit_form(session: Session, csrf: CsrfToken, mut db: Db) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;

    let subs: Vec<Subreddit> = subreddits::table
        .order(subreddits::name.asc())
        .select(Subreddit::as_select())
        .load(&mut *db)
        .await?;

    Ok(layout(
        "Submit Post",
        current_user.as_deref(),
        Some(csrf.token()),
        html! {
            div class="max-w-2xl mx-auto" {
                h1 class="text-2xl font-bold mb-6" { "Create a Post" }
                form action="/submit" method="post"
                     class="space-y-4 bg-white rounded-lg shadow p-6" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    div {
                        label for="subreddit_id" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Community"
                        }
                        select id="subreddit_id" name="subreddit_id" required
                               class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                      focus:outline-none focus:ring-2 focus:ring-orange-400" {
                            option value="" disabled selected { "Choose a community..." }
                            @for sub in &subs {
                                option value=(sub.id) { "r/" (sub.name) }
                            }
                        }
                    }
                    div {
                        label for="title" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Title"
                        }
                        input type="text" id="title" name="title" required
                              maxlength="300"
                              placeholder="An interesting title"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    div {
                        label for="url" class="block text-sm font-medium text-gray-700 mb-1" {
                            "URL " span class="text-gray-400" { "(optional)" }
                        }
                        input type="url" id="url" name="url"
                              placeholder="https://example.com"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    div {
                        label for="body" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Text " span class="text-gray-400" { "(optional for link posts)" }
                        }
                        textarea id="body" name="body" rows="8"
                                 placeholder="What's on your mind?"
                                 class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                        focus:outline-none focus:ring-2 focus:ring-orange-400" {}
                    }
                    button type="submit"
                           class="w-full bg-orange-500 text-white py-2 rounded font-medium \
                                  hover:bg-orange-600 transition-colors" {
                        "Post"
                    }
                }
            }
        },
    ))
}

/// Submit form for a specific subreddit
#[secured]
#[get("/r/{slug}/submit")]
pub async fn submit_to_sub_form(
    Path(slug): Path<String>,
    session: Session,
    csrf: CsrfToken,
    mut db: Db,
) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;

    let sub: Subreddit = subreddits::table
        .filter(subreddits::slug.eq(&slug))
        .select(Subreddit::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg(format!("r/{slug} not found")))?;

    Ok(layout(
        &format!("Submit to r/{}", sub.name),
        current_user.as_deref(),
        Some(csrf.token()),
        html! {
            div class="max-w-2xl mx-auto" {
                h1 class="text-2xl font-bold mb-6" {
                    "Post to "
                    span class="text-orange-600" { "r/" (sub.name) }
                }
                form action="/submit" method="post"
                     class="space-y-4 bg-white rounded-lg shadow p-6" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    input type="hidden" name="subreddit_id" value=(sub.id);
                    div {
                        label for="title" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Title"
                        }
                        input type="text" id="title" name="title" required
                              maxlength="300"
                              placeholder="An interesting title"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    div {
                        label for="url" class="block text-sm font-medium text-gray-700 mb-1" {
                            "URL " span class="text-gray-400" { "(optional)" }
                        }
                        input type="url" id="url" name="url"
                              placeholder="https://example.com"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    div {
                        label for="body" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Text"
                        }
                        textarea id="body" name="body" rows="8"
                                 placeholder="What's on your mind?"
                                 class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                        focus:outline-none focus:ring-2 focus:ring-orange-400" {}
                    }
                    button type="submit"
                           class="w-full bg-orange-500 text-white py-2 rounded font-medium \
                                  hover:bg-orange-600 transition-colors" {
                        "Post"
                    }
                }
            }
        },
    ))
}

#[derive(serde::Deserialize)]
pub struct SubmitPostForm {
    pub subreddit_id: i64,
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub body: String,
}

#[secured]
#[post("/submit")]
pub async fn submit(
    session: Session,
    mut db: Db,
    form: Form<SubmitPostForm>,
) -> AutumnResult<Markup> {
    let user_id: i64 = session
        .get("user_id")
        .await
        .ok_or_else(|| AutumnError::unauthorized_msg("Login required"))?
        .parse()
        .map_err(|_| AutumnError::bad_request_msg("Invalid session"))?;

    let title = form.0.title.trim().to_string();
    if title.is_empty() || title.len() > 300 {
        return Err(AutumnError::unprocessable_msg(
            "Title must be 1-300 characters",
        ));
    }

    let base_slug = slugify(&title);
    if base_slug.is_empty() {
        return Err(AutumnError::unprocessable_msg(
            "Title must contain at least one letter or number",
        ));
    }
    let url = if form.0.url.trim().is_empty() {
        None
    } else {
        Some(form.0.url.trim().to_string())
    };

    // Look up the subreddit slug for redirect
    let sub: Subreddit = subreddits::table
        .find(form.0.subreddit_id)
        .select(Subreddit::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Subreddit not found"))?;

    // Ensure unique slug within this subreddit by appending a suffix
    let slug = unique_slug(&base_slug, form.0.subreddit_id, &mut db).await?;

    // Insert the post, then create an explicit author upvote so
    // score always matches the sum of actual vote rows.
    let post_id: i64 = diesel::insert_into(posts::table)
        .values((
            posts::title.eq(&title),
            posts::slug.eq(&slug),
            posts::body.eq(form.0.body.trim()),
            posts::url.eq(&url),
            posts::author_id.eq(user_id),
            posts::subreddit_id.eq(form.0.subreddit_id),
            posts::score.eq(1_i64),
        ))
        .returning(posts::id)
        .get_result(&mut *db)
        .await?;

    diesel::insert_into(crate::schema::votes::table)
        .values((
            crate::schema::votes::user_id.eq(user_id),
            crate::schema::votes::post_id.eq(post_id),
            crate::schema::votes::value.eq(1_i16),
        ))
        .execute(&mut *db)
        .await?;

    Ok(redirect_to(&format!("/r/{}", sub.slug)))
}

// ── View single post with comments ─────────────────────────────

#[allow(clippy::too_many_lines)] // Template-heavy function
#[get("/r/{sub_slug}/posts/{post_slug}")]
pub async fn show(
    Path((sub_slug, post_slug)): Path<(String, String)>,
    session: Session,
    csrf: CsrfToken,
    mut db: Db,
) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;
    let current_user_id = session.get("user_id").await;

    let sub: Subreddit = subreddits::table
        .filter(subreddits::slug.eq(&sub_slug))
        .select(Subreddit::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg(format!("r/{sub_slug} not found")))?;

    let post: Post = posts::table
        .filter(posts::slug.eq(&post_slug))
        .filter(posts::subreddit_id.eq(sub.id))
        .select(Post::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Post not found"))?;

    let author: User = users::table
        .find(post.author_id)
        .select(User::as_select())
        .first(&mut *db)
        .await?;

    // Load top-level comments with authors
    let post_comments: Vec<(crate::models::Comment, String)> = comments::table
        .filter(comments::post_id.eq(post.id))
        .filter(comments::parent_id.is_null())
        .inner_join(users::table.on(comments::author_id.eq(users::id)))
        .order(comments::score.desc())
        .select((crate::models::Comment::as_select(), users::username))
        .load(&mut *db)
        .await?;

    let is_author = current_user_id
        .as_ref()
        .and_then(|id| id.parse::<i64>().ok())
        .is_some_and(|id| id == post.author_id);

    Ok(layout(
        &post.title,
        current_user.as_deref(),
        Some(csrf.token()),
        html! {
            // Breadcrumbs
            div class="text-sm text-gray-500 mb-4" {
                a href=(format!("/r/{}", sub.slug)) class="hover:text-orange-600" {
                    "r/" (sub.name)
                }
                " \u{203A} Post"
            }

            // Post card
            div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6" {
                div class="flex items-start gap-4" {
                    (vote_controls(post.id, post.score))
                    div class="flex-1" {
                        h1 class="text-2xl font-bold text-gray-900 mb-2" { (post.title) }
                        div class="text-xs text-gray-400 mb-4" {
                            "posted by "
                            a href=(format!("/u/{}", author.username))
                               class="text-gray-500 hover:underline" {
                                "u/" (author.username)
                            }
                            " " (time_ago(&post.created_at))
                        }
                        @if let Some(ref url) = post.url {
                            a href=(url) target="_blank" rel="noopener noreferrer"
                               class="text-blue-600 hover:underline text-sm mb-3 block" {
                                (url)
                                " \u{2197}"
                            }
                        }
                        @if !post.body.is_empty() {
                            div class="prose max-w-none text-gray-700" {
                                @for para in post.body.split("\n\n") {
                                    @if !para.trim().is_empty() {
                                        p { (para.trim()) }
                                    }
                                }
                            }
                        }
                        @if is_author {
                            div class="flex gap-3 mt-4 pt-4 border-t border-gray-100 text-sm" {
                                a href=(format!("/r/{}/posts/{}/edit", sub.slug, post.slug))
                                   class="text-gray-500 hover:text-orange-600" { "Edit" }
                                button
                                    hx-delete=(format!("/r/{}/posts/{}", sub.slug, post.slug))
                                    hx-confirm="Delete this post? This cannot be undone."
                                    class="text-red-500 hover:text-red-700 cursor-pointer" {
                                    "Delete"
                                }
                            }
                        }
                    }
                }
            }

            // Comment form
            @if current_user.is_some() {
                form action=(format!("/r/{}/posts/{}/comments", sub.slug, post.slug))
                     method="post"
                     class="bg-white rounded-lg shadow-sm border border-gray-200 p-4 mb-6" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    textarea name="body" rows="4" required
                             placeholder="What are your thoughts?"
                             class="w-full border border-gray-300 rounded px-3 py-2 text-sm mb-3 \
                                    focus:outline-none focus:ring-2 focus:ring-orange-400" {}
                    button type="submit"
                           class="px-4 py-2 bg-orange-500 text-white rounded text-sm \
                                  hover:bg-orange-600" {
                        "Comment"
                    }
                }
            } @else {
                div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4 mb-6 \
                           text-center text-sm text-gray-500" {
                    a href="/login" class="text-orange-600 hover:underline" { "Log in" }
                    " to comment"
                }
            }

            // Comments
            div class="space-y-3" {
                h2 class="font-semibold text-gray-700 mb-2" {
                    (post.comment_count) " Comments"
                }
                @for (comment, comment_author) in &post_comments {
                    div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4" {
                        div class="flex items-center gap-2 text-xs text-gray-400 mb-2" {
                            a href=(format!("/u/{comment_author}"))
                               class="font-medium text-gray-600 hover:underline" {
                                "u/" (comment_author)
                            }
                            "\u{2022} " (time_ago(&comment.created_at))
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
                    p class="text-gray-400 text-center py-8 text-sm" {
                        "No comments yet. Start the conversation!"
                    }
                }
            }
        },
    ))
}

// ── Edit post ──────────────────────────────────────────────────

#[secured]
#[get("/r/{sub_slug}/posts/{post_slug}/edit")]
pub async fn edit_form(
    Path((sub_slug, post_slug)): Path<(String, String)>,
    session: Session,
    csrf: CsrfToken,
    mut db: Db,
) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;
    let user_id: i64 = session
        .get("user_id")
        .await
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);

    let post: Post = posts::table
        .inner_join(subreddits::table.on(posts::subreddit_id.eq(subreddits::id)))
        .filter(subreddits::slug.eq(&sub_slug))
        .filter(posts::slug.eq(&post_slug))
        .select(Post::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Post not found"))?;

    if post.author_id != user_id {
        return Err(AutumnError::forbidden_msg(
            "You can only edit your own posts",
        ));
    }

    Ok(layout(
        &format!("Edit: {}", post.title),
        current_user.as_deref(),
        Some(csrf.token()),
        html! {
            div class="max-w-2xl mx-auto" {
                h1 class="text-2xl font-bold mb-6" { "Edit Post" }
                form action=(format!("/r/{sub_slug}/posts/{post_slug}")) method="post"
                     class="space-y-4 bg-white rounded-lg shadow p-6" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    div {
                        label for="title" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Title"
                        }
                        input type="text" id="title" name="title" required
                              value=(post.title)
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    div {
                        label for="body" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Text"
                        }
                        textarea id="body" name="body" rows="8"
                                 class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                        focus:outline-none focus:ring-2 focus:ring-orange-400" {
                            (post.body)
                        }
                    }
                    div class="flex gap-3" {
                        button type="submit"
                               class="px-6 py-2 bg-orange-500 text-white rounded font-medium \
                                      hover:bg-orange-600 transition-colors" {
                            "Save"
                        }
                        a href=(format!("/r/{sub_slug}/posts/{post_slug}"))
                           class="px-6 py-2 text-gray-500 hover:text-gray-700" { "Cancel" }
                    }
                }
            }
        },
    ))
}

#[derive(serde::Deserialize)]
pub struct EditPostForm {
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[secured]
#[post("/r/{sub_slug}/posts/{post_slug}")]
pub async fn update(
    Path((sub_slug, post_slug)): Path<(String, String)>,
    session: Session,
    mut db: Db,
    form: Form<EditPostForm>,
) -> AutumnResult<Markup> {
    let user_id: i64 = session
        .get("user_id")
        .await
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);

    let post: Post = posts::table
        .inner_join(subreddits::table.on(posts::subreddit_id.eq(subreddits::id)))
        .filter(subreddits::slug.eq(&sub_slug))
        .filter(posts::slug.eq(&post_slug))
        .select(Post::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Post not found"))?;

    if post.author_id != user_id {
        return Err(AutumnError::forbidden_msg(
            "You can only edit your own posts",
        ));
    }

    let title = form.0.title.trim().to_string();
    if title.is_empty() || title.len() > 300 {
        return Err(AutumnError::unprocessable_msg(
            "Title must be 1-300 characters",
        ));
    }

    let base_slug = slugify(&title);
    if base_slug.is_empty() {
        return Err(AutumnError::unprocessable_msg(
            "Title must contain at least one letter or number",
        ));
    }
    // Ensure unique slug within subreddit, excluding the current post
    let new_slug = unique_slug_excluding(&base_slug, post.subreddit_id, post.id, &mut db).await?;

    diesel::update(posts::table.find(post.id))
        .set((
            posts::title.eq(&title),
            posts::slug.eq(&new_slug),
            posts::body.eq(form.0.body.trim()),
            posts::updated_at.eq(chrono::Utc::now().naive_utc()),
        ))
        .execute(&mut *db)
        .await?;

    Ok(redirect_to(&format!("/r/{sub_slug}/posts/{new_slug}")))
}

// ── Delete post (htmx) ────────────────────────────────────────

#[secured]
#[delete("/r/{sub_slug}/posts/{post_slug}")]
pub async fn delete_post(
    Path((sub_slug, post_slug)): Path<(String, String)>,
    session: Session,
    mut db: Db,
) -> AutumnResult<autumn_web::reexports::axum::response::Response> {
    let user_id: i64 = session
        .get("user_id")
        .await
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);

    let post: Post = posts::table
        .inner_join(subreddits::table.on(posts::subreddit_id.eq(subreddits::id)))
        .filter(subreddits::slug.eq(&sub_slug))
        .filter(posts::slug.eq(&post_slug))
        .select(Post::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Post not found"))?;

    if post.author_id != user_id {
        return Err(AutumnError::forbidden_msg(
            "You can only delete your own posts",
        ));
    }

    diesel::delete(posts::table.find(post.id))
        .execute(&mut *db)
        .await?;

    Ok(super::layout::hx_redirect_to(&format!("/r/{sub_slug}")))
}

// ── Helpers ────────────────────────────────────────────────────

/// Generate a unique slug within a subreddit by appending `-2`, `-3`, etc.
async fn unique_slug(
    base: &str,
    subreddit_id: i64,
    conn: &mut diesel_async::AsyncPgConnection,
) -> AutumnResult<String> {
    let mut slug = base.to_string();
    let mut suffix = 2u64;
    loop {
        let count: i64 = posts::table
            .filter(posts::subreddit_id.eq(subreddit_id))
            .filter(posts::slug.eq(&slug))
            .count()
            .get_result(conn)
            .await?;
        if count == 0 {
            return Ok(slug);
        }
        slug = format!("{base}-{suffix}");
        suffix += 1;
    }
}

/// Like `unique_slug`, but excludes a specific post ID (for updates).
async fn unique_slug_excluding(
    base: &str,
    subreddit_id: i64,
    exclude_id: i64,
    conn: &mut diesel_async::AsyncPgConnection,
) -> AutumnResult<String> {
    let mut slug = base.to_string();
    let mut suffix = 2u64;
    loop {
        let count: i64 = posts::table
            .filter(posts::subreddit_id.eq(subreddit_id))
            .filter(posts::slug.eq(&slug))
            .filter(posts::id.ne(exclude_id))
            .count()
            .get_result(conn)
            .await?;
        if count == 0 {
            return Ok(slug);
        }
        slug = format!("{base}-{suffix}");
        suffix += 1;
    }
}
