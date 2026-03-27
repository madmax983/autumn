// ── v0.2 Feature: #[scheduled] macro ────────────────────────────
//
// Declares a background task that runs every hour alongside the
// HTTP server. Dependencies (AppState) are injected automatically,
// just like handler extractors.
//
// Errors are logged at WARN level and the task retries on the
// next scheduled interval.

use autumn_web::prelude::*;

#[scheduled(every = "1h", name = "link-checker")]
pub async fn check_links(state: AppState) -> AutumnResult<()> {
    tracing::info!("link-checker: scanning bookmarks for dead links");

    // In a real app, this would:
    // 1. Load all bookmarks where alive = true
    // 2. HTTP HEAD each URL
    // 3. Mark unreachable ones as alive = false
    //
    // For the example, we just log — no HTTP client dependency needed.
    let _ = state;
    tracing::info!("link-checker: scan complete");

    Ok(())
}
