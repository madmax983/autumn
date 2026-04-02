//! Subreddit routes — list communities, create, and show.
//!
//! Demonstrates: #[secured] macro for requiring authentication,
//! repository-generated CRUD, `CsrfToken` for forms, Maud templates.

use autumn_web::extract::Path;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::NewSubreddit;
use crate::repositories::{PgSubredditRepository, SubredditRepository};
use crate::schema::{subreddits, users};
use crate::slugify::slugify;

use super::layout::{layout, redirect_to, time_ago};

// ── List all communities ───────────────────────────────────────

#[get("/r")]
pub async fn list(session: Session, repo: PgSubredditRepository) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;
    let all = repo.find_all().await?;

    Ok(layout(
        "Communities",
        current_user.as_deref(),
        html! {
            div class="flex justify-between items-center mb-6" {
                h1 class="text-2xl font-bold" { "Communities" }
                @if current_user.is_some() {
                    a href="/r/create"
                      class="px-4 py-2 bg-orange-500 text-white rounded hover:bg-orange-600 text-sm" {
                        "+ Create Community"
                    }
                }
            }
            div class="space-y-3" {
                @for sub in &all {
                    a href=(format!("/r/{}", sub.slug))
                       class="block bg-white rounded-lg shadow-sm border border-gray-200 \
                              hover:border-orange-300 hover:shadow transition-all p-4" {
                        div class="flex items-center justify-between" {
                            div {
                                h2 class="font-semibold text-orange-600" { "r/" (sub.name) }
                                @if !sub.description.is_empty() {
                                    p class="text-sm text-gray-500 mt-1" { (sub.description) }
                                }
                            }
                            div class="text-right text-xs text-gray-400" {
                                div { (sub.subscriber_count) " members" }
                                div { "Created " (time_ago(&sub.created_at)) }
                            }
                        }
                    }
                }
                @if all.is_empty() {
                    p class="text-gray-400 text-center py-12" {
                        "No communities yet. Be the first to create one!"
                    }
                }
            }
        },
    ))
}

// ── Create community form (requires auth) ──────────────────────

#[secured]
#[get("/r/create")]
pub async fn create_form(session: Session, csrf: CsrfToken) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;
    Ok(layout(
        "Create Community",
        current_user.as_deref(),
        html! {
            div class="max-w-lg mx-auto" {
                h1 class="text-2xl font-bold mb-6" { "Create a Community" }
                form action="/r/create" method="post"
                     class="space-y-4 bg-white rounded-lg shadow p-6" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    div {
                        label for="name" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Community Name"
                        }
                        div class="flex items-center" {
                            span class="text-gray-400 mr-1" { "r/" }
                            input type="text" id="name" name="name" required
                                  minlength="2" maxlength="32"
                                  placeholder="rustlang"
                                  pattern="[a-zA-Z0-9_]+"
                                  class="flex-1 border border-gray-300 rounded px-3 py-2 text-sm \
                                         focus:outline-none focus:ring-2 focus:ring-orange-400";
                        }
                        p class="text-xs text-gray-400 mt-1" {
                            "Letters, numbers, and underscores only"
                        }
                    }
                    div {
                        label for="description" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Description"
                        }
                        textarea id="description" name="description" rows="3"
                                 placeholder="What is this community about?"
                                 class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                        focus:outline-none focus:ring-2 focus:ring-orange-400" {}
                    }
                    button type="submit"
                           class="w-full bg-orange-500 text-white py-2 rounded font-medium \
                                  hover:bg-orange-600 transition-colors" {
                        "Create Community"
                    }
                }
            }
        },
    ))
}

#[derive(serde::Deserialize)]
pub struct CreateSubredditForm {
    pub name: String,
    pub description: String,
}

#[secured]
#[post("/r/create")]
pub async fn create(
    session: Session,
    mut db: Db,
    form: Form<CreateSubredditForm>,
) -> AutumnResult<Markup> {
    let user_id: i64 = session
        .get("user_id")
        .await
        .ok_or_else(|| AutumnError::unauthorized_msg("Login required"))?
        .parse()
        .map_err(|_| AutumnError::bad_request_msg("Invalid session"))?;

    let name = form.0.name.trim().to_string();
    let slug = slugify(&name);

    let new_sub = NewSubreddit {
        name: name.clone(),
        slug: slug.clone(),
        description: form.0.description.trim().to_string(),
        creator_id: user_id,
    };

    diesel::insert_into(subreddits::table)
        .values(&new_sub)
        .execute(&mut *db)
        .await
        .map_err(|_| AutumnError::unprocessable_msg("Community name already taken"))?;

    Ok(redirect_to(&format!("/r/{slug}")))
}

// ── Show subreddit with posts ──────────────────────────────────

#[get("/r/{slug}")]
pub async fn show(
    Path(slug): Path<String>,
    session: Session,
    repo: PgSubredditRepository,
    mut db: Db,
) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;

    let subs = repo.find_by_slug(slug.clone()).await?;
    let sub = subs
        .into_iter()
        .next()
        .ok_or_else(|| AutumnError::not_found_msg(format!("r/{slug} not found")))?;

    // Load posts for this subreddit ordered by hot_rank
    let posts: Vec<(i64, String, String, i64, i64, String, chrono::NaiveDateTime)> =
        crate::schema::posts::table
            .filter(crate::schema::posts::subreddit_id.eq(sub.id))
            .inner_join(users::table.on(crate::schema::posts::author_id.eq(users::id)))
            .order(crate::schema::posts::hot_rank.desc())
            .select((
                crate::schema::posts::id,
                crate::schema::posts::title,
                crate::schema::posts::slug,
                crate::schema::posts::score,
                crate::schema::posts::comment_count,
                users::username,
                crate::schema::posts::created_at,
            ))
            .load(&mut *db)
            .await?;

    Ok(layout(
        &format!("r/{}", sub.name),
        current_user.as_deref(),
        html! {
            // Subreddit header
            div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6" {
                div class="flex justify-between items-start" {
                    div {
                        h1 class="text-2xl font-bold text-orange-600" { "r/" (sub.name) }
                        @if !sub.description.is_empty() {
                            p class="text-gray-600 mt-2" { (sub.description) }
                        }
                        p class="text-xs text-gray-400 mt-2" {
                            (sub.subscriber_count) " members \u{2022} created "
                            (time_ago(&sub.created_at))
                        }
                    }
                    @if current_user.is_some() {
                        a href=(format!("/r/{}/submit", sub.slug))
                          class="px-4 py-2 bg-orange-500 text-white rounded text-sm \
                                 hover:bg-orange-600" {
                            "New Post"
                        }
                    }
                }
            }

            // Post list
            div class="space-y-2" {
                @for (post_id, title, post_slug, score, comment_count, author, created_at) in &posts {
                    div class="bg-white rounded-lg shadow-sm border border-gray-200 \
                               hover:border-orange-300 transition-colors" {
                        div class="flex items-start gap-3 p-4" {
                            // Vote controls
                            (super::layout::vote_controls(*post_id, *score))

                            // Post info
                            div class="flex-1 min-w-0" {
                                a href=(format!("/r/{}/posts/{}", sub.slug, post_slug))
                                   class="text-lg font-medium text-gray-900 hover:text-orange-600" {
                                    (title)
                                }
                                div class="text-xs text-gray-400 mt-1" {
                                    "posted by "
                                    a href=(format!("/u/{author}"))
                                       class="text-gray-500 hover:underline" { "u/" (author) }
                                    " " (time_ago(created_at))
                                    " \u{2022} "
                                    a href=(format!("/r/{}/posts/{}", sub.slug, post_slug))
                                       class="text-gray-500 hover:text-orange-600" {
                                        (comment_count) " comments"
                                    }
                                }
                            }
                        }
                    }
                }
                @if posts.is_empty() {
                    p class="text-gray-400 text-center py-12" {
                        "No posts yet. Be the first!"
                    }
                }
            }
        },
    ))
}
