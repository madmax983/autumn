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
/// Uses `ON CONFLICT DO UPDATE` so that concurrent/rapid requests always
/// persist the latest intended value. Score is recomputed in a single
/// `UPDATE ... SET score = (subquery)` so no read-then-write race exists.
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
    // All mutations filter by (user_id, post_id) — never by vote_id —
    // so concurrent toggle/flip requests cannot target a deleted row.
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
            // New vote — ON CONFLICT DO UPDATE so rapid clicks always
            // persist the latest intended value instead of dropping one.
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

    // Recompute score atomically in a single statement — no read-then-write race.
    // Uses raw SQL because Diesel doesn't support SET col = (subquery) directly.
    diesel::sql_query(
        "UPDATE posts SET score = COALESCE((SELECT SUM(value::bigint) FROM votes WHERE post_id = $1), 0) WHERE id = $1"
    )
    .bind::<diesel::sql_types::BigInt, _>(post_id)
    .execute(&mut **db)
    .await?;

    let score: i64 = posts::table
        .find(post_id)
        .select(posts::score)
        .first(&mut **db)
        .await?;

    Ok(vote_controls(post_id, score))
}
