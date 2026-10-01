//! Redis dead-letter retention contract for issue #3055.
//!
//! The Redis job backend used to trim its `{prefix}:dead` list to a hard-coded
//! 1_000 entries and delete the trimmed jobs' metadata keys, silently. The
//! retention is now configurable via `jobs.redis.dead_letter_limit`
//! (`0` = unbounded), and every trim emits a `warn!` plus increments the
//! `autumn_jobs_dead_letter_trimmed_total` counter.
//!
//! These tests prove, against a real Redis:
//!
//! - `dead_letter_limit = 3`: dead-lettering 5 jobs keeps exactly 3 entries,
//!   deletes the 2 trimmed jobs' per-id metadata keys, and counts 2 on the
//!   trim metric (the `warn!` fires on the same code path as the increment).
//! - `dead_letter_limit = 0`: dead-lettering 5 jobs keeps all 5 — the trim is
//!   skipped entirely.
//!
//! Requires Docker (testcontainers Redis) and is marked `#[ignore]`. Run:
//!
//! ```text
//! cargo test -p autumn-web --features redis --test integration_tests redis_dead_letter_limit -- --ignored
//! ```
//!
//! Gated on `#[cfg(feature = "redis")]` so it always compiles.

#![cfg(feature = "redis")]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use autumn_web::config::{JobConfig, JobRedisConfig};
use autumn_web::job::{self, JobInfo};
use autumn_web::{AppState, AutumnError, AutumnResult, metrics};
use serde_json::Value;
use tokio::time::{sleep, timeout};

static DLQ_HANDLER_CALLS: AtomicUsize = AtomicUsize::new(0);

fn always_fails(
    _state: AppState,
    _payload: Value,
) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
    Box::pin(async move {
        DLQ_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst);
        Err(AutumnError::internal_server_error_msg("boom"))
    })
}

fn dlq_job_info() -> JobInfo {
    // max_attempts = 1: the first failure dead-letters immediately, no backoff.
    JobInfo::new("dlq_probe", 1, 1, always_fails)
}

fn dlq_config(url: &str, dead_letter_limit: usize) -> JobConfig {
    JobConfig {
        backend: "redis".to_owned(),
        workers: 2,
        redis: JobRedisConfig {
            url: Some(url.to_owned()),
            dead_letter_limit,
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn start_redis() -> (
    testcontainers::ContainerAsync<testcontainers_modules::redis::Redis>,
    String,
) {
    use testcontainers::runners::AsyncRunner as _;
    use testcontainers_modules::redis::Redis as RedisImage;

    let container = RedisImage::default()
        .start()
        .await
        .expect("start Redis container");
    let port = container
        .get_host_port_ipv4(6379)
        .await
        .expect("redis port");
    (container, format!("redis://127.0.0.1:{port}"))
}

/// Current total of the trim counter (process-global; read as a delta around
/// each test so concurrent tests in this binary can't skew the assertion).
fn trimmed_total() -> u64 {
    metrics::snapshot()
        .iter()
        .find(|instrument| instrument.name == "autumn_jobs_dead_letter_trimmed_total")
        .map(|instrument| {
            instrument
                .series
                .iter()
                .map(|series| match &series.value {
                    metrics::SeriesValue::Counter { value } => *value,
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

async fn dead_letter_five_jobs(dead_letter_limit: usize) -> (usize, usize, u64) {
    use redis::AsyncCommands as _;

    let (_container, url) = start_redis().await;
    let _guard = job::global_job_runtime_test_lock().lock().await;
    job::clear_global_job_client();
    DLQ_HANDLER_CALLS.store(0, Ordering::SeqCst);
    let before = trimmed_total();

    let config = dlq_config(&url, dead_letter_limit);
    let state = AppState::for_test().with_profile("dev");
    let shutdown = tokio_util::sync::CancellationToken::new();
    job::start_runtime(vec![dlq_job_info()], &state, &shutdown, &config, true)
        .expect("worker runtime should start");

    for _ in 0..5 {
        job::enqueue("dlq_probe", serde_json::json!({}))
            .await
            .expect("enqueue dlq_probe job");
    }

    // All five handlers must run (each fails once and dead-letters).
    timeout(Duration::from_secs(15), async {
        loop {
            if DLQ_HANDLER_CALLS.load(Ordering::SeqCst) >= 5 {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("all five jobs should fail");

    let client = autumn_web::redis_tls::open_client(&url).expect("open redis client");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("connect to redis");
    let dead_key = format!("{}:dead", config.redis.key_prefix);

    // Wait for the settles to land; the dead-letter count stabilizes once all
    // five trims (or non-trims) have applied.
    let expected_len = if dead_letter_limit == 0 {
        5
    } else {
        dead_letter_limit.min(5)
    };
    timeout(Duration::from_secs(15), async {
        loop {
            let len: usize = conn.llen(&dead_key).await.expect("llen dead list");
            if len == expected_len {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("dead-letter list should settle at the retention limit");

    let len: usize = conn.llen(&dead_key).await.expect("llen dead list");
    // Per-id metadata keys of trimmed jobs must be gone; survivors keep theirs.
    let metadata_keys: Vec<String> = conn
        .keys(format!("{}:dead-record:*", config.redis.key_prefix))
        .await
        .expect("list dead-record keys");
    let trimmed_delta = trimmed_total() - before;

    shutdown.cancel();
    job::clear_global_job_client();
    (len, metadata_keys.len(), trimmed_delta)
}

/// A configured limit caps the dead list, deletes trimmed jobs' metadata, and
/// counts every trimmed entry on the metric.
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn redis_dead_letter_limit_trims_oldest_and_counts_trimmed() {
    let (len, metadata_keys, trimmed_delta) = dead_letter_five_jobs(3).await;
    assert_eq!(
        len, 3,
        "dead-letter list should be capped at the configured limit"
    );
    assert_eq!(
        metadata_keys, 3,
        "trimmed jobs' per-id metadata keys must be deleted with them"
    );
    assert_eq!(
        trimmed_delta, 2,
        "the two trims (4th and 5th dead-letters) should count on autumn_jobs_dead_letter_trimmed_total"
    );
}

/// `dead_letter_limit = 0` means unbounded: no trim, no metric, nothing lost.
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn redis_dead_letter_limit_zero_never_trims() {
    let (len, metadata_keys, trimmed_delta) = dead_letter_five_jobs(0).await;
    assert_eq!(len, 5, "unbounded dead-letter list should keep every entry");
    assert_eq!(metadata_keys, 5, "no metadata keys should be deleted");
    assert_eq!(
        trimmed_delta, 0,
        "no trim should be counted when the limit is 0"
    );
}
