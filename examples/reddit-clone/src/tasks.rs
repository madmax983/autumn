// ── v0.2 Feature: #[scheduled] macro ────────────────────────────
//
// Background task that recalculates the hot-rank score for posts
// every 15 minutes. Uses a simplified version of Reddit's hot
// ranking algorithm based on score and age.

use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use autumn_web::prelude::*;

use crate::schema::posts;

/// Recalculate `hot_rank` for all posts using a time-decay formula.
///
/// `hot_rank` = score / (`age_in_hours` + 2) ^ 1.5
///
/// This ensures fresh posts with engagement bubble up, while older
/// posts naturally decay off the front page.
#[scheduled(every = "15m", name = "hot-rank-calculator")]
pub async fn recalculate_hot_ranks(state: AppState) -> AutumnResult<()> {
    let pool = state
        .pool()
        .ok_or_else(|| AutumnError::service_unavailable_msg("No database pool"))?;

    let mut conn = pool.get().await.map_err(AutumnError::from)?;

    // Load all posts with their scores and timestamps
    let all_posts: Vec<(i64, i64, chrono::NaiveDateTime)> = posts::table
        .select((posts::id, posts::score, posts::created_at))
        .load(&mut conn)
        .await?;

    if all_posts.is_empty() {
        tracing::info!("hot-rank: no posts to rank");
        return Ok(());
    }

    let now = chrono::Utc::now().naive_utc();
    let mut updated = 0u64;

    for (id, score, created_at) in &all_posts {
        #[allow(clippy::cast_precision_loss)] // Acceptable for ranking math
        let age_hours = (now - *created_at).num_seconds() as f64 / 3600.0;
        #[allow(clippy::cast_precision_loss)]
        let hot_rank = *score as f64 / (age_hours + 2.0_f64).powf(1.5);

        diesel::update(posts::table.find(*id))
            .set(posts::hot_rank.eq(hot_rank))
            .execute(&mut conn)
            .await?;

        updated += 1;
    }

    tracing::info!("hot-rank: recalculated {updated} posts");
    Ok(())
}
