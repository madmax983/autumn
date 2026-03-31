// ── v0.2 Feature: #[scheduled] macro ─────────────────────────────────────
//
// Declares a background task that runs every hour alongside the
// HTTP server. Dependencies (AppState) are injected automatically,
// just like handler extractors.
//
// Errors are logged at WARN level and the task retries on the
// next scheduled interval.

use autumn_web::prelude::*;
use futures::FutureExt;
use reqwest::StatusCode;

use crate::repositories::BookmarkRepository;

fn response_is_reachable(status: StatusCode) -> bool {
    status.is_success() || status.is_redirection()
}

fn head_requires_get_fallback(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
    )
}

fn probe_outcome(head: Result<StatusCode, ()>, get: Option<Result<StatusCode, ()>>) -> bool {
    match head {
        Ok(status) if response_is_reachable(status) => true,
        Ok(status) if head_requires_get_fallback(status) => {
            get.is_some_and(|fallback| fallback.is_ok_and(response_is_reachable))
        }
        _ => false,
    }
}

async fn probe_reachable(client: &reqwest::Client, url: &str) -> bool {
    let head = client
        .head(url)
        .send()
        .await
        .map(|response| response.status());
    match head {
        Ok(status) if response_is_reachable(status) => true,
        Ok(status) if head_requires_get_fallback(status) => {
            let get = client
                .get(url)
                .send()
                .await
                .map(|response| response.status());
            probe_outcome(Ok(status), Some(get.map_err(|_| ())))
        }
        Ok(status) => probe_outcome(Ok(status), None),
        Err(_) => false,
    }
}

async fn process_shard(
    repo: &BookmarkRepository,
    client: &reqwest::Client,
    shard: u32,
) -> AutumnResult<(u32, u32)> {
    let shard_alive = repo.find_alive_in_shard(shard).await?;

    if shard_alive.is_empty() {
        return Ok((0, 0));
    }
    let shard_checked_count =
        u32::try_from(shard_alive.len()).expect("shard bookmark count must fit in u32");

    tracing::info!(shard, count = shard_alive.len(), "link-checker owns shard");

    let mut dead_count = 0u32;
    for (id, url) in shard_alive {
        let reachable = probe_reachable(client, &url).await;

        if !reachable {
            tracing::warn!("link-checker: dead link id={id} url={url}");
            if repo.mark_dead(id).await? {
                dead_count += 1;
            }
        }
    }

    Ok((shard_checked_count, dead_count))
}

#[scheduled(every = "1h", name = "link-checker")]
pub async fn check_links(_state: AppState) -> AutumnResult<()> {
    let repo = BookmarkRepository;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AutumnError::from(std::io::Error::other(e.to_string())))?;

    let mut dead_count = 0u32;
    let mut checked_count = 0u32;
    let mut owned_shards = 0u32;

    for shard in BookmarkRepository::shard_ids() {
        let Some(lease) = BookmarkRepository::acquire_shard_lease(shard).await? else {
            tracing::debug!(shard, "link-checker shard already owned by another replica");
            continue;
        };

        let result = std::panic::AssertUnwindSafe(process_shard(&repo, &client, shard))
            .catch_unwind()
            .await;
        let release_result = BookmarkRepository::release_shard_lease(lease).await;

        match (result, release_result) {
            (Ok(Ok((shard_checked_count, shard_dead_count))), Ok(())) => {
                owned_shards += 1;
                dead_count += shard_dead_count;
                checked_count += shard_checked_count;
            }
            (Ok(Err(err)), Ok(())) => return Err(err),
            (Ok(Ok(_)), Err(err)) => return Err(err),
            (Ok(Err(err)), Err(release_err)) => {
                tracing::warn!(release_error = %release_err, "link-checker shard release failed after shard error");
                return Err(err);
            }
            (Err(panic), Ok(())) => std::panic::resume_unwind(panic),
            (Err(panic), Err(release_err)) => {
                tracing::error!(release_error = %release_err, "link-checker shard release failed after panic");
                std::panic::resume_unwind(panic);
            }
        }
    }

    tracing::info!(
        owned_shards,
        dead_count,
        checked = checked_count,
        "link-checker done"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{head_requires_get_fallback, probe_outcome, response_is_reachable};
    use reqwest::StatusCode;

    #[test]
    fn reachable_statuses_match_link_checker_expectation() {
        assert!(response_is_reachable(StatusCode::OK));
        assert!(response_is_reachable(StatusCode::MOVED_PERMANENTLY));
        assert!(!response_is_reachable(StatusCode::NOT_FOUND));
    }

    #[test]
    fn head_fallback_is_limited_to_head_unsupported_statuses() {
        assert!(head_requires_get_fallback(StatusCode::METHOD_NOT_ALLOWED));
        assert!(head_requires_get_fallback(StatusCode::NOT_IMPLEMENTED));
        assert!(!head_requires_get_fallback(StatusCode::NOT_FOUND));
        assert!(!head_requires_get_fallback(StatusCode::FORBIDDEN));
    }

    #[test]
    fn successful_head_probe_marks_link_reachable() {
        assert!(probe_outcome(Ok(StatusCode::OK), None));
    }

    #[test]
    fn head_405_falls_back_to_get_before_marking_dead() {
        assert!(probe_outcome(
            Ok(StatusCode::METHOD_NOT_ALLOWED),
            Some(Ok(StatusCode::OK))
        ));
        assert!(!probe_outcome(
            Ok(StatusCode::METHOD_NOT_ALLOWED),
            Some(Ok(StatusCode::NOT_FOUND))
        ));
    }

    #[test]
    fn hard_head_failures_do_not_trigger_fallback() {
        assert!(!probe_outcome(Ok(StatusCode::NOT_FOUND), None));
        assert!(!probe_outcome(Err(()), None));
    }
}
