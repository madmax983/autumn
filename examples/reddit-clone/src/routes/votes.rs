//! Vote routes — upvote and downvote posts via htmx.
//!
//! Demonstrates: htmx-powered partial page updates, session-based
//! authentication, upsert with ON CONFLICT, returning updated HTML
//! fragments instead of full pages.

use autumn_web::extract::Path;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::schema::{posts, votes};

use super::layout::vote_controls;

/// Upvote a post (+1). Returns updated vote controls HTML via htmx.
#[post("/posts/{post_id}/upvote")]
pub async fn upvote(
    Path(post_id): Path<i64>,
    session: Session,
    mut db: Db,
) -> AutumnResult<Markup> {
    cast_vote(post_id, 1, &session, &mut db).await
}

/// Downvote a post (-1). Returns updated vote controls HTML via htmx.
#[post("/posts/{post_id}/downvote")]
pub async fn downvote(
    Path(post_id): Path<i64>,
    session: Session,
    mut db: Db,
) -> AutumnResult<Markup> {
    cast_vote(post_id, -1, &session, &mut db).await
}

/// Cast a vote on a post. Handles insert-or-update and score recalculation.
///
/// Score is always recomputed from the votes table (not incremented),
/// so concurrent requests cannot cause drift.
async fn cast_vote(
    post_id: i64,
    value: i16,
    session: &Session,
    db: &mut Db,
) -> AutumnResult<Markup> {
    let user_id: i64 = session
        .get("user_id")
        .await
        .ok_or_else(|| AutumnError::unauthorized_msg("Login required to vote"))?
        .parse()
        .map_err(|_| AutumnError::bad_request_msg("Invalid session"))?;

    // Check if user already voted on this post
    let existing: Option<(i64, i16)> = votes::table
        .filter(votes::user_id.eq(user_id))
        .filter(votes::post_id.eq(post_id))
        .select((votes::id, votes::value))
        .first(&mut **db)
        .await
        .optional()?;

    match existing {
        Some((vote_id, old_value)) if old_value == value => {
            // Same vote again — toggle off (remove vote)
            diesel::delete(votes::table.find(vote_id))
                .execute(&mut **db)
                .await?;
        }
        Some((vote_id, _)) => {
            // Different vote — flip direction
            diesel::update(votes::table.find(vote_id))
                .set(votes::value.eq(value))
                .execute(&mut **db)
                .await?;
        }
        None => {
            // New vote — use ON CONFLICT to handle race conditions
            diesel::insert_into(votes::table)
                .values((
                    votes::user_id.eq(user_id),
                    votes::post_id.eq(post_id),
                    votes::value.eq(value),
                ))
                .on_conflict((votes::user_id, votes::post_id))
                .do_nothing()
                .execute(&mut **db)
                .await?;
        }
    }

    // Recompute score from actual votes — avoids drift from concurrent requests.
    // Load all vote values and sum in Rust to sidestep Diesel's Numeric type.
    let all_votes: Vec<i16> = votes::table
        .filter(votes::post_id.eq(post_id))
        .select(votes::value)
        .load(&mut **db)
        .await?;
    let score: i64 = all_votes.iter().map(|&v| i64::from(v)).sum();

    diesel::update(posts::table.find(post_id))
        .set(posts::score.eq(score))
        .execute(&mut **db)
        .await?;

    Ok(vote_controls(post_id, score))
}
