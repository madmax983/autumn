//! Vote routes — upvote and downvote posts via htmx.
//!
//! Demonstrates: htmx-powered partial page updates, session-based
//! authentication, upsert with ON CONFLICT, returning updated HTML
//! fragments instead of full pages.

use autumn_web::extract::Path;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::Post;
use crate::schema::{posts, votes};

use super::layout::vote_controls;

/// Upvote a post (+1). Returns updated vote controls HTML via htmx.
#[post("/posts/{post_id}/upvote")]
pub async fn upvote(
    Path(post_id): Path<i64>,
    session: Session,
    mut db: Db,
    State(state): State<AppState>,
) -> AutumnResult<Markup> {
    cast_vote(post_id, 1, &session, &mut db, &state).await
}

/// Downvote a post (-1). Returns updated vote controls HTML via htmx.
#[post("/posts/{post_id}/downvote")]
pub async fn downvote(
    Path(post_id): Path<i64>,
    session: Session,
    mut db: Db,
    State(state): State<AppState>,
) -> AutumnResult<Markup> {
    cast_vote(post_id, -1, &session, &mut db, &state).await
}

/// Cast a vote on a post. Handles insert-or-update and score recalculation.
async fn cast_vote(
    post_id: i64,
    value: i16,
    session: &Session,
    db: &mut Db,
    state: &AppState,
) -> AutumnResult<Markup> {
    let user_id: i64 = session
        .get("user_id")
        .await
        .ok_or_else(|| AutumnError::unauthorized_msg("Login required to vote"))?
        .parse()
        .map_err(|_| AutumnError::bad_request_msg("Invalid session"))?;

    // Verify the post exists before touching votes
    let post_exists: bool = diesel::dsl::select(diesel::dsl::exists(posts::table.find(post_id)))
        .get_result(&mut **db)
        .await?;
    if !post_exists {
        return Err(AutumnError::not_found_msg("Post not found"));
    }

    // Check if user already voted on this post
    let existing_value: Option<i16> = votes::table
        .filter(votes::user_id.eq(user_id))
        .filter(votes::post_id.eq(post_id))
        .select(votes::value)
        .first(&mut **db)
        .await
        .optional()?;

    match existing_value {
        Some(old_value) if old_value == value => {
            // Same vote again — toggle off (remove vote)
            diesel::delete(
                votes::table
                    .filter(votes::user_id.eq(user_id))
                    .filter(votes::post_id.eq(post_id)),
            )
            .execute(&mut **db)
            .await?;
        }
        Some(_) => {
            // Different vote — flip direction
            diesel::update(
                votes::table
                    .filter(votes::user_id.eq(user_id))
                    .filter(votes::post_id.eq(post_id)),
            )
            .set(votes::value.eq(value))
            .execute(&mut **db)
            .await?;
        }
        None => {
            // New vote
            diesel::insert_into(votes::table)
                .values((
                    votes::user_id.eq(user_id),
                    votes::post_id.eq(post_id),
                    votes::value.eq(value),
                ))
                .on_conflict((votes::user_id, votes::post_id))
                .do_update()
                .set(votes::value.eq(value))
                .execute(&mut **db)
                .await?;
        }
    }

    // Recompute score atomically using sum of vote values
    let new_score: i64 = votes::table
        .filter(votes::post_id.eq(post_id))
        .select(diesel::dsl::sum(votes::value))
        .first::<Option<i64>>(&mut **db)
        .await?
        .unwrap_or(0);

    // Update the score on the post in database
    diesel::update(posts::table.find(post_id))
        .set(posts::score.eq(new_score))
        .execute(&mut **db)
        .await?;

    // Load the updated post to broadcast it
    let post: Post = posts::table.find(post_id).first(&mut **db).await?;

    // Load the subreddit to get its slug
    let sub: crate::models::Subreddit = crate::schema::subreddits::table
        .find(post.subreddit_id)
        .first(&mut **db)
        .await?;

    // Broadcast the update so all SSE clients see the new score live
    let _ = state.broadcast().publish_oob(
        "posts",
        &post.dom_id(),
        &autumn_web::htmx::OobSwap::OuterHTML,
        &post.render_fragment(),
    );

    let _ = state.broadcast().publish_oob(
        &format!("posts:r/{}", sub.slug),
        &post.dom_id(),
        &autumn_web::htmx::OobSwap::OuterHTML,
        &post.render_fragment(),
    );

    Ok(vote_controls(post_id, new_score))
}

autumn_web::paths![upvote, downvote];
