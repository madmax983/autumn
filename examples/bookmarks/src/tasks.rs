// ── v0.2 Feature: #[scheduled] macro ────────────────────────────
//
// Declares a background task that runs every hour alongside the
// HTTP server. Dependencies (AppState) are injected automatically,
// just like handler extractors.
//
// Errors are logged at WARN level and the task retries on the
// next scheduled interval.

use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use autumn_web::prelude::*;

use crate::schema::bookmarks;

#[scheduled(every = "1h", name = "link-checker")]
pub async fn check_links(state: AppState) -> AutumnResult<()> {
    let pool = state
        .pool
        .as_ref()
        .ok_or_else(|| AutumnError::service_unavailable_msg("No database pool"))?;

    let mut conn = pool.get().await.map_err(AutumnError::from)?;

    // Load all bookmarks currently marked alive
    let alive: Vec<(i64, String)> = bookmarks::table
        .filter(bookmarks::alive.eq(true))
        .select((bookmarks::id, bookmarks::url))
        .load(&mut conn)
        .await?;

    if alive.is_empty() {
        tracing::info!("link-checker: no alive bookmarks to check");
        return Ok(());
    }

    tracing::info!("link-checker: checking {} URLs", alive.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AutumnError::from(std::io::Error::other(e.to_string())))?;

    let mut dead_count = 0u32;
    for (id, url) in &alive {
        let reachable = client
            .head(url)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success() || r.status().is_redirection());

        if !reachable {
            tracing::warn!("link-checker: dead link id={id} url={url}");
            diesel::update(bookmarks::table.find(id))
                .set(bookmarks::alive.eq(false))
                .execute(&mut conn)
                .await?;
            dead_count += 1;
        }
    }

    tracing::info!(
        "link-checker: done — {dead_count} dead of {} checked",
        alive.len()
    );
    Ok(())
}
