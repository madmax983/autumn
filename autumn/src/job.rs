//! On-demand background job infrastructure.
//!
//! Provides [`JobInfo`] metadata used by `#[job]` and `jobs![]`, plus local
//! and Redis-backed queue backends.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use futures::FutureExt as _;
#[cfg(feature = "redis")]
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AppState, AutumnError, AutumnResult};

pub type JobHandler =
    fn(AppState, Value) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>>;

#[derive(Clone)]
pub struct JobInfo {
    pub name: String,
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub handler: JobHandler,
}

#[derive(Clone)]
pub struct JobClient {
    local_sender: Option<tokio::sync::mpsc::Sender<QueuedJob>>,
    #[cfg(feature = "redis")]
    redis: Option<RedisClient>,
    registry: crate::actuator::JobRegistry,
    default_max_attempts: u32,
    default_initial_backoff_ms: u64,
    per_job_defaults: HashMap<String, (u32, u64)>,
}

#[derive(Debug)]
struct QueuedJob {
    name: String,
    payload: Value,
    attempt: u32,
    max_attempts: u32,
    initial_backoff_ms: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum JobExecutionOutcome {
    Succeeded,
    Failed(String),
    Panicked(String),
}

#[cfg(feature = "redis")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RedisJobRecord {
    id: String,
    name: String,
    payload: Value,
    attempt: u32,
    max_attempts: u32,
    initial_backoff_ms: u64,
    #[serde(default)]
    claimed_by: Option<String>,
    #[serde(default)]
    claimed_at_ms: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
}

#[cfg(all(feature = "redis", test))]
#[derive(Debug, Clone)]
struct RedisClaimedRecord {
    record: RedisJobRecord,
    deadline_ms: u64,
}

#[cfg(feature = "redis")]
#[derive(Debug, Clone)]
struct RedisRetrySchedule {
    record: RedisJobRecord,
    due_at_ms: u64,
}

#[cfg(feature = "redis")]
#[derive(Debug, Clone)]
enum RedisFailureAction {
    Retry(RedisRetrySchedule),
    DeadLetter(RedisJobRecord),
}

#[cfg(feature = "redis")]
#[derive(Debug, Clone)]
enum RedisStaleRecovery {
    Requeue(RedisJobRecord),
    DeadLetter(RedisJobRecord),
}

#[cfg(feature = "redis")]
struct RedisMaintenanceThrottle {
    next_run_at: std::time::Instant,
    interval: std::time::Duration,
}

#[cfg(feature = "redis")]
impl RedisMaintenanceThrottle {
    const fn new(now: std::time::Instant, interval: std::time::Duration) -> Self {
        Self {
            next_run_at: now,
            interval,
        }
    }

    fn take_due(&mut self, now: std::time::Instant) -> bool {
        if now < self.next_run_at {
            return false;
        }
        self.next_run_at = now + self.interval;
        true
    }
}

#[cfg(feature = "redis")]
const REDIS_STALE_MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

#[cfg(feature = "redis")]
const REDIS_WORKER_IDLE_SLEEP_MAX: std::time::Duration = std::time::Duration::from_millis(200);

#[cfg(feature = "redis")]
fn redis_retry_promotion_interval_ms(default_backoff_ms: u64, jobs: &[JobInfo]) -> u64 {
    let mut interval_ms = default_backoff_ms.max(1);
    for job in jobs {
        if job.initial_backoff_ms > 0 {
            interval_ms = interval_ms.min(job.initial_backoff_ms);
        }
    }
    interval_ms
}

#[cfg(feature = "redis")]
fn redis_worker_idle_sleep(retry_promotion_interval: std::time::Duration) -> std::time::Duration {
    retry_promotion_interval.min(REDIS_WORKER_IDLE_SLEEP_MAX)
}

#[cfg(feature = "redis")]
#[derive(Clone)]
struct RedisWorkerConfig {
    queue_key: String,
    processing_key: String,
    delayed_key: String,
    dead_key: String,
    record_prefix: String,
    worker_id: String,
    visibility_timeout_ms: u64,
    default_attempts: u32,
    default_backoff: u64,
    retry_promotion_interval: std::time::Duration,
}

static GLOBAL_JOB_CLIENT: OnceLock<RwLock<Option<Arc<JobClient>>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn global_job_runtime_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn run_job_handler(
    handler: JobHandler,
    state: AppState,
    payload: Value,
) -> JobExecutionOutcome {
    let future = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (handler)(state, payload)
    })) {
        Ok(future) => future,
        Err(panic) => return JobExecutionOutcome::Panicked(format_job_panic(panic.as_ref())),
    };

    match std::panic::AssertUnwindSafe(future).catch_unwind().await {
        Ok(Ok(())) => JobExecutionOutcome::Succeeded,
        Ok(Err(error)) => JobExecutionOutcome::Failed(error.to_string()),
        Err(panic) => JobExecutionOutcome::Panicked(format_job_panic(panic.as_ref())),
    }
}

fn format_job_panic(panic: &(dyn std::any::Any + Send)) -> String {
    let detail = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&'static str>().copied())
        .unwrap_or("non-string panic payload");
    format!("job handler panicked: {detail}")
}

#[must_use]
pub fn global_job_client() -> Option<Arc<JobClient>> {
    GLOBAL_JOB_CLIENT
        .get()
        .and_then(|lock| lock.read().ok().and_then(|guard| guard.clone()))
}

pub(crate) fn init_global_job_client(client: JobClient) {
    if let Some(lock) = GLOBAL_JOB_CLIENT.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = Some(Arc::new(client));
        }
        return;
    }
    let _ = GLOBAL_JOB_CLIENT.set(RwLock::new(Some(Arc::new(client))));
}

pub(crate) fn clear_global_job_client() {
    if let Some(lock) = GLOBAL_JOB_CLIENT.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = None;
        }
    } else {
        let _ = GLOBAL_JOB_CLIENT.set(RwLock::new(None));
    }
}

/// Enqueue a job payload on the configured runtime backend.
///
/// # Errors
///
/// Returns an internal error when the jobs runtime is not initialized, when
/// `name` does not match a registered job, or when the active backend rejects
/// the enqueue operation.
pub async fn enqueue(name: &str, payload: Value) -> AutumnResult<()> {
    let Some(client) = global_job_client() else {
        return Err(AutumnError::internal_server_error(std::io::Error::other(
            "job runtime is not initialized; register jobs with AppBuilder::jobs()",
        )));
    };
    client.enqueue(name, payload).await
}

impl JobClient {
    /// Enqueue a job by name with a JSON payload.
    ///
    /// # Errors
    ///
    /// Returns an internal error when `name` does not match a registered job
    /// or enqueueing fails in the active backend.
    pub async fn enqueue(&self, name: &str, payload: Value) -> AutumnResult<()> {
        let Some((job_max_attempts, job_backoff_ms)) = self.per_job_defaults.get(name).copied()
        else {
            return Err(AutumnError::internal_server_error(std::io::Error::other(
                format!("job '{name}' is not registered; add it to AppBuilder::jobs()"),
            )));
        };
        let job_max_attempts = if job_max_attempts != 0 {
            job_max_attempts
        } else {
            self.default_max_attempts
        };
        let job_backoff_ms = if job_backoff_ms != 0 {
            job_backoff_ms
        } else {
            self.default_initial_backoff_ms
        };
        self.registry.record_enqueue(name);

        if let Some(sender) = &self.local_sender {
            sender
                .send(QueuedJob {
                    name: name.to_string(),
                    payload,
                    attempt: 1,
                    max_attempts: job_max_attempts,
                    initial_backoff_ms: job_backoff_ms,
                })
                .await
                .map_err(|e| {
                    AutumnError::internal_server_error(std::io::Error::other(format!(
                        "failed to enqueue job: {e}"
                    )))
                })
        } else {
            #[cfg(feature = "redis")]
            {
                if let Some(redis) = &self.redis {
                    return redis
                        .enqueue(name, payload, job_max_attempts, job_backoff_ms)
                        .await;
                }
            }
            Err(AutumnError::internal_server_error(std::io::Error::other(
                "job runtime backend is unavailable",
            )))
        }
    }
}

pub(crate) fn start_runtime(
    jobs: Vec<JobInfo>,
    state: &AppState,
    shutdown: &tokio_util::sync::CancellationToken,
    config: &crate::config::JobConfig,
) -> AutumnResult<()> {
    validate_unique_job_names(&jobs).map_err(|error| {
        AutumnError::internal_server_error(std::io::Error::other(format!(
            "invalid jobs configuration: {error}"
        )))
    })?;

    match config.backend.as_str() {
        "local" => {
            start_local_runtime(
                jobs,
                state,
                shutdown,
                config.workers,
                config.max_attempts,
                config.initial_backoff_ms,
            );
            Ok(())
        }
        "redis" => {
            #[cfg(feature = "redis")]
            {
                start_redis_runtime(jobs, state, shutdown, config)
            }
            #[cfg(not(feature = "redis"))]
            {
                let _ = jobs;
                let _ = state;
                let _ = shutdown;
                let _ = config;
                Err(AutumnError::internal_server_error(std::io::Error::other(
                    "jobs.backend=redis requested but redis feature is disabled",
                )))
            }
        }
        other => {
            tracing::warn!(backend = %other, "unknown jobs backend; falling back to local backend");
            start_local_runtime(
                jobs,
                state,
                shutdown,
                config.workers,
                config.max_attempts,
                config.initial_backoff_ms,
            );
            Ok(())
        }
    }
}

pub(crate) fn start_local_runtime(
    jobs: Vec<JobInfo>,
    state: &AppState,
    shutdown: &tokio_util::sync::CancellationToken,
    workers: usize,
    default_max_attempts: u32,
    default_initial_backoff_ms: u64,
) {
    let per_job_defaults = build_per_job_defaults(&jobs);
    let jobs_by_name: Arc<RwLock<HashMap<String, JobInfo>>> = Arc::new(RwLock::new(
        jobs.into_iter().map(|j| (j.name.clone(), j)).collect(),
    ));

    {
        let guard = jobs_by_name.read().expect("job registry lock poisoned");
        for name in guard.keys() {
            state.job_registry.register(name);
        }
    }

    let worker_count = workers.max(1);
    let (tx, rx) = tokio::sync::mpsc::channel::<QueuedJob>(1024);
    let shared_rx = Arc::new(tokio::sync::Mutex::new(rx));

    let client = JobClient {
        local_sender: Some(tx.clone()),
        #[cfg(feature = "redis")]
        redis: None,
        registry: state.job_registry.clone(),
        default_max_attempts,
        default_initial_backoff_ms,
        per_job_defaults,
    };
    init_global_job_client(client);

    for _ in 0..worker_count {
        let state = state.clone();
        let tx = tx.clone();
        let jobs_by_name = Arc::clone(&jobs_by_name);
        let shared_rx = Arc::clone(&shared_rx);
        let shutdown = shutdown.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    maybe = async {
                        let mut guard = shared_rx.lock().await;
                        guard.recv().await
                    } => {
                        let Some(job) = maybe else { break; };
                        execute_local_job(job, &jobs_by_name, &tx, &state).await;
                    }
                }
            }
        });
    }
}

async fn execute_local_job(
    job: QueuedJob,
    jobs_by_name: &Arc<RwLock<HashMap<String, JobInfo>>>,
    tx: &tokio::sync::mpsc::Sender<QueuedJob>,
    state: &AppState,
) {
    state.job_registry.record_start(&job.name);

    let Some((handler, info_max_attempts, info_backoff_ms)) = jobs_by_name
        .read()
        .expect("job registry lock poisoned")
        .get(&job.name)
        .map(|info| (info.handler, info.max_attempts, info.initial_backoff_ms))
    else {
        state
            .job_registry
            .record_failure(&job.name, format!("unknown job '{}'", job.name), true);
        return;
    };
    let max_attempts = if job.max_attempts != 0 {
        job.max_attempts
    } else if info_max_attempts != 0 {
        info_max_attempts
    } else {
        5
    };
    let backoff_ms = if job.initial_backoff_ms != 0 {
        job.initial_backoff_ms
    } else if info_backoff_ms != 0 {
        info_backoff_ms
    } else {
        250
    };

    match run_job_handler(handler, state.clone(), job.payload.clone()).await {
        JobExecutionOutcome::Succeeded => state.job_registry.record_success(&job.name),
        JobExecutionOutcome::Failed(error) => {
            if job.attempt < max_attempts {
                state
                    .job_registry
                    .record_retry(&job.name, &error, job.attempt);
                let sender = tx.clone();
                let registry = state.job_registry.clone();
                let name = job.name.clone();
                let payload = job.payload;
                let delay = backoff_ms.saturating_mul(2_u64.saturating_pow(job.attempt - 1));
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    registry.record_enqueue(&name);
                    let _ = sender
                        .send(QueuedJob {
                            name,
                            payload,
                            attempt: job.attempt + 1,
                            max_attempts,
                            initial_backoff_ms: backoff_ms,
                        })
                        .await;
                });
            } else {
                state.job_registry.record_failure(&job.name, error, true);
            }
        }
        JobExecutionOutcome::Panicked(error) => {
            tracing::error!(job = %job.name, error = %error, "local job handler panicked");
            state.job_registry.record_failure(&job.name, error, true);
        }
    }
}

#[cfg(feature = "redis")]
#[derive(Clone)]
struct RedisClient {
    connection: redis::aio::ConnectionManager,
    queue_key: String,
    record_prefix: String,
}

#[cfg(feature = "redis")]
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(feature = "redis")]
fn redis_record_key(record_prefix: &str, id: &str) -> String {
    format!("{record_prefix}{id}")
}

#[cfg(feature = "redis")]
const fn redis_retry_delay_ms(initial_backoff_ms: u64, attempt: u32) -> u64 {
    initial_backoff_ms.saturating_mul(2_u64.saturating_pow(attempt.saturating_sub(1)))
}

#[cfg(feature = "redis")]
fn clear_redis_claim(record: &mut RedisJobRecord) {
    record.claimed_by = None;
    record.claimed_at_ms = None;
}

#[cfg(all(feature = "redis", test))]
fn claim_redis_record(
    mut record: RedisJobRecord,
    worker_id: &str,
    now_ms: u64,
    visibility_timeout_ms: u64,
) -> RedisClaimedRecord {
    record.claimed_by = Some(worker_id.to_string());
    record.claimed_at_ms = Some(now_ms);
    RedisClaimedRecord {
        record,
        deadline_ms: now_ms.saturating_add(visibility_timeout_ms),
    }
}

#[cfg(feature = "redis")]
fn prepare_redis_failure_action(
    mut record: RedisJobRecord,
    error: String,
    now_ms: u64,
) -> RedisFailureAction {
    clear_redis_claim(&mut record);
    record.last_error = Some(error);

    if record.attempt < record.max_attempts {
        let due_at_ms = now_ms.saturating_add(redis_retry_delay_ms(
            record.initial_backoff_ms,
            record.attempt,
        ));
        record.attempt = record.attempt.saturating_add(1);
        RedisFailureAction::Retry(RedisRetrySchedule { record, due_at_ms })
    } else {
        RedisFailureAction::DeadLetter(record)
    }
}

#[cfg(feature = "redis")]
fn prepare_redis_panic_dead_letter(mut record: RedisJobRecord, error: String) -> RedisJobRecord {
    clear_redis_claim(&mut record);
    record.last_error = Some(error);
    record
}

#[cfg(feature = "redis")]
fn recover_stale_redis_record(
    mut record: RedisJobRecord,
    now_ms: u64,
    visibility_timeout_ms: u64,
) -> Option<RedisStaleRecovery> {
    let claimed_at_ms = record.claimed_at_ms?;
    if claimed_at_ms.saturating_add(visibility_timeout_ms) > now_ms {
        return None;
    }

    let claimed_by = record
        .claimed_by
        .clone()
        .unwrap_or_else(|| "unknown worker".to_string());
    record.last_error = Some(format!(
        "visibility timeout expired for claim by {claimed_by} at {claimed_at_ms}"
    ));
    clear_redis_claim(&mut record);

    if record.attempt < record.max_attempts {
        record.attempt = record.attempt.saturating_add(1);
        Some(RedisStaleRecovery::Requeue(record))
    } else {
        Some(RedisStaleRecovery::DeadLetter(record))
    }
}

#[cfg(feature = "redis")]
fn encode_redis_record(record: &RedisJobRecord) -> AutumnResult<String> {
    serde_json::to_string(record).map_err(|e| {
        AutumnError::internal_server_error(std::io::Error::other(format!(
            "serialize durable job failed: {e}"
        )))
    })
}

#[cfg(feature = "redis")]
impl RedisClient {
    async fn enqueue(
        &self,
        name: &str,
        payload: Value,
        default_max_attempts: u32,
        default_initial_backoff_ms: u64,
    ) -> AutumnResult<()> {
        let mut connection = self.connection.clone();
        let id = uuid::Uuid::new_v4().to_string();
        let msg = RedisJobRecord {
            id: id.clone(),
            name: name.to_string(),
            payload,
            attempt: 1,
            max_attempts: default_max_attempts,
            initial_backoff_ms: default_initial_backoff_ms,
            claimed_by: None,
            claimed_at_ms: None,
            last_error: None,
        };
        let encoded = encode_redis_record(&msg)?;
        let record_key = redis_record_key(&self.record_prefix, &id);

        redis::pipe()
            .atomic()
            .cmd("SET")
            .arg(record_key)
            .arg(encoded)
            .ignore()
            .cmd("LPUSH")
            .arg(&self.queue_key)
            .arg(id)
            .ignore()
            .query_async::<()>(&mut connection)
            .await
            .map_err(|e| {
                AutumnError::internal_server_error(std::io::Error::other(format!(
                    "enqueue durable job failed: {e}"
                )))
            })
    }
}

#[cfg(feature = "redis")]
fn new_redis_connection_manager(
    client: &redis::Client,
    label: &str,
) -> Result<redis::aio::ConnectionManager, AutumnError> {
    use redis::aio::ConnectionManagerConfig;

    redis::aio::ConnectionManager::new_lazy_with_config(
        client.clone(),
        ConnectionManagerConfig::new(),
    )
    .map_err(|e| {
        AutumnError::internal_server_error(std::io::Error::other(format!(
            "failed to create {label}: {e}"
        )))
    })
}

#[cfg(feature = "redis")]
async fn push_json_list_item<T: ?Sized + Serialize + Sync>(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    value: &T,
) {
    use redis::AsyncCommands as _;

    if let Ok(encoded) = serde_json::to_string(value) {
        let _ = connection.lpush::<_, _, ()>(key, encoded).await;
    }
}

#[cfg(feature = "redis")]
async fn claim_next_redis_job(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
) -> Result<Option<RedisJobRecord>, redis::RedisError> {
    const CLAIM_SCRIPT: &str = r"
local id = redis.call('RPOP', KEYS[1])
if not id then
  return nil
end
local key = KEYS[3] .. id
local body = redis.call('GET', key)
if not body then
  return nil
end
local ok, record = pcall(cjson.decode, body)
if not ok then
  redis.call('ZADD', KEYS[2], ARGV[3], id)
  return { id, body }
end
record['claimed_by'] = ARGV[1]
record['claimed_at_ms'] = tonumber(ARGV[2])
local updated = cjson.encode(record)
redis.call('SET', key, updated)
redis.call('ZADD', KEYS[2], ARGV[3], id)
return { id, updated }
";

    let now_ms = now_unix_ms();
    let deadline_ms = now_ms.saturating_add(worker_config.visibility_timeout_ms);
    let response: Option<(String, String)> = redis::cmd("EVAL")
        .arg(CLAIM_SCRIPT)
        .arg(3)
        .arg(&worker_config.queue_key)
        .arg(&worker_config.processing_key)
        .arg(&worker_config.record_prefix)
        .arg(&worker_config.worker_id)
        .arg(now_ms)
        .arg(deadline_ms)
        .query_async(connection)
        .await?;

    let Some((id, body)) = response else {
        return Ok(None);
    };

    match serde_json::from_str::<RedisJobRecord>(&body) {
        Ok(record) => Ok(Some(record)),
        Err(error) => {
            tracing::warn!(job_id = %id, error = %error, "invalid durable job record");
            let malformed_id = id.clone();
            let malformed = serde_json::json!({
                "id": id,
                "error": error.to_string(),
                "raw_payload": body,
            });
            push_json_list_item(connection, &worker_config.dead_key, &malformed).await;
            let _ = redis::cmd("ZREM")
                .arg(&worker_config.processing_key)
                .arg(malformed_id)
                .query_async::<usize>(connection)
                .await;
            Ok(None)
        }
    }
}

#[cfg(feature = "redis")]
async fn record_enqueues_for_redis_ids(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    ids: &[String],
) -> Result<(), redis::RedisError> {
    if ids.is_empty() {
        return Ok(());
    }

    let keys: Vec<String> = ids
        .iter()
        .map(|id| redis_record_key(&worker_config.record_prefix, id))
        .collect();
    let bodies: Vec<Option<String>> = redis::cmd("MGET")
        .arg(&keys)
        .query_async(connection)
        .await?;

    for body in bodies.into_iter().flatten() {
        if let Ok(record) = serde_json::from_str::<RedisJobRecord>(&body) {
            state.job_registry.record_enqueue(&record.name);
        }
    }

    Ok(())
}

#[cfg(feature = "redis")]
async fn promote_due_redis_retries(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
) -> Result<(), redis::RedisError> {
    const PROMOTE_SCRIPT: &str = r"
local ids = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, ARGV[2])
local promoted = {}
for _, id in ipairs(ids) do
  if redis.call('ZREM', KEYS[1], id) == 1 then
    redis.call('LPUSH', KEYS[2], id)
    table.insert(promoted, id)
  end
end
return promoted
";

    let promoted: Vec<String> = redis::cmd("EVAL")
        .arg(PROMOTE_SCRIPT)
        .arg(2)
        .arg(&worker_config.delayed_key)
        .arg(&worker_config.queue_key)
        .arg(now_unix_ms())
        .arg(64_usize)
        .query_async(connection)
        .await?;

    record_enqueues_for_redis_ids(connection, worker_config, state, &promoted).await?;
    Ok(())
}

#[cfg(feature = "redis")]
fn expected_claim_args(record: &RedisJobRecord) -> Option<(&str, u64)> {
    Some((record.claimed_by.as_deref()?, record.claimed_at_ms?))
}

#[cfg(feature = "redis")]
async fn apply_claimed_redis_transition(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    expected: &RedisJobRecord,
    mode: &str,
    encoded_record: Option<String>,
    due_at_ms: Option<u64>,
) -> Result<bool, redis::RedisError> {
    const TRANSITION_SCRIPT: &str = r"
local key = KEYS[2] .. ARGV[1]
local body = redis.call('GET', key)
if not body then
  return 0
end
local ok, record = pcall(cjson.decode, body)
if not ok then
  return 0
end
if record['claimed_by'] ~= ARGV[2] then
  return 0
end
if record['claimed_at_ms'] ~= tonumber(ARGV[3]) then
  return 0
end
redis.call('ZREM', KEYS[1], ARGV[1])
if ARGV[4] == 'success' then
  redis.call('DEL', key)
elseif ARGV[4] == 'retry' then
  redis.call('SET', key, ARGV[5])
  redis.call('ZADD', KEYS[3], ARGV[6], ARGV[1])
elseif ARGV[4] == 'dead' then
  redis.call('LPUSH', KEYS[4], ARGV[5])
  redis.call('DEL', key)
else
  return 0
end
return 1
";

    let Some((claimed_by, claimed_at_ms)) = expected_claim_args(expected) else {
        return Ok(false);
    };

    let applied: usize = redis::cmd("EVAL")
        .arg(TRANSITION_SCRIPT)
        .arg(4)
        .arg(&worker_config.processing_key)
        .arg(&worker_config.record_prefix)
        .arg(&worker_config.delayed_key)
        .arg(&worker_config.dead_key)
        .arg(&expected.id)
        .arg(claimed_by)
        .arg(claimed_at_ms)
        .arg(mode)
        .arg(encoded_record.unwrap_or_default())
        .arg(due_at_ms.unwrap_or_default())
        .query_async(connection)
        .await?;

    Ok(applied == 1)
}

#[cfg(feature = "redis")]
async fn ack_redis_success(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    record: &RedisJobRecord,
) -> Result<bool, redis::RedisError> {
    apply_claimed_redis_transition(connection, worker_config, record, "success", None, None).await
}

#[cfg(feature = "redis")]
async fn schedule_redis_retry(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    expected: &RedisJobRecord,
    schedule: &RedisRetrySchedule,
) -> Result<bool, redis::RedisError> {
    let Ok(encoded) = encode_redis_record(&schedule.record) else {
        tracing::warn!(job_id = %schedule.record.id, "failed to serialize redis retry record");
        return Ok(false);
    };
    apply_claimed_redis_transition(
        connection,
        worker_config,
        expected,
        "retry",
        Some(encoded),
        Some(schedule.due_at_ms),
    )
    .await
}

#[cfg(feature = "redis")]
async fn dead_letter_redis_job(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    expected: &RedisJobRecord,
    record: &RedisJobRecord,
) -> Result<bool, redis::RedisError> {
    let Ok(encoded) = encode_redis_record(record) else {
        tracing::warn!(job_id = %record.id, "failed to serialize redis dead-letter record");
        return Ok(false);
    };
    apply_claimed_redis_transition(
        connection,
        worker_config,
        expected,
        "dead",
        Some(encoded),
        None,
    )
    .await
}

#[cfg(feature = "redis")]
async fn apply_stale_redis_recovery(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    expected: &RedisJobRecord,
    action: &RedisStaleRecovery,
) -> Result<bool, redis::RedisError> {
    const STALE_SCRIPT: &str = r"
local key = KEYS[2] .. ARGV[1]
local body = redis.call('GET', key)
if not body then
  redis.call('ZREM', KEYS[1], ARGV[1])
  return 0
end
local ok, record = pcall(cjson.decode, body)
if not ok then
  redis.call('ZREM', KEYS[1], ARGV[1])
  return 0
end
if record['claimed_by'] ~= ARGV[2] then
  return 0
end
if record['claimed_at_ms'] ~= tonumber(ARGV[3]) then
  return 0
end
redis.call('ZREM', KEYS[1], ARGV[1])
if ARGV[4] == 'requeue' then
  redis.call('SET', key, ARGV[5])
  redis.call('LPUSH', KEYS[3], ARGV[1])
elseif ARGV[4] == 'dead' then
  redis.call('LPUSH', KEYS[4], ARGV[5])
  redis.call('DEL', key)
else
  return 0
end
return 1
";

    let Some((claimed_by, claimed_at_ms)) = expected_claim_args(expected) else {
        return Ok(false);
    };
    let (mode, record) = match action {
        RedisStaleRecovery::Requeue(record) => ("requeue", record),
        RedisStaleRecovery::DeadLetter(record) => ("dead", record),
    };
    let Ok(encoded) = encode_redis_record(record) else {
        tracing::warn!(job_id = %record.id, "failed to serialize stale redis record");
        return Ok(false);
    };

    let applied: usize = redis::cmd("EVAL")
        .arg(STALE_SCRIPT)
        .arg(4)
        .arg(&worker_config.processing_key)
        .arg(&worker_config.record_prefix)
        .arg(&worker_config.queue_key)
        .arg(&worker_config.dead_key)
        .arg(&expected.id)
        .arg(claimed_by)
        .arg(claimed_at_ms)
        .arg(mode)
        .arg(encoded)
        .query_async(connection)
        .await?;

    Ok(applied == 1)
}

#[cfg(feature = "redis")]
async fn recover_stale_redis_jobs(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
) -> Result<(), redis::RedisError> {
    let stale_ids: Vec<String> = redis::cmd("ZRANGEBYSCORE")
        .arg(&worker_config.processing_key)
        .arg("-inf")
        .arg(now_unix_ms())
        .arg("LIMIT")
        .arg(0)
        .arg(64)
        .query_async(connection)
        .await?;

    if stale_ids.is_empty() {
        return Ok(());
    }

    let keys: Vec<String> = stale_ids
        .iter()
        .map(|id| redis_record_key(&worker_config.record_prefix, id))
        .collect();
    let bodies: Vec<Option<String>> = redis::cmd("MGET")
        .arg(&keys)
        .query_async(connection)
        .await?;

    for (id, body) in stale_ids.into_iter().zip(bodies) {
        let Some(body) = body else {
            let _ = redis::cmd("ZREM")
                .arg(&worker_config.processing_key)
                .arg(&id)
                .query_async::<usize>(connection)
                .await?;
            continue;
        };
        let Ok(record) = serde_json::from_str::<RedisJobRecord>(&body) else {
            let _ = redis::cmd("ZREM")
                .arg(&worker_config.processing_key)
                .arg(&id)
                .query_async::<usize>(connection)
                .await?;
            continue;
        };
        let Some(action) = recover_stale_redis_record(
            record.clone(),
            now_unix_ms(),
            worker_config.visibility_timeout_ms,
        ) else {
            continue;
        };

        if apply_stale_redis_recovery(connection, worker_config, &record, &action).await? {
            match &action {
                RedisStaleRecovery::Requeue(requeued) => {
                    if let Some(error) = requeued.last_error.as_deref() {
                        state
                            .job_registry
                            .record_retry(&requeued.name, error, record.attempt);
                    }
                    state.job_registry.record_enqueue(&requeued.name);
                }
                RedisStaleRecovery::DeadLetter(dead) => {
                    state.job_registry.record_failure(
                        &dead.name,
                        dead.last_error
                            .clone()
                            .unwrap_or_else(|| "visibility timeout expired".to_string()),
                        true,
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(feature = "redis")]
fn spawn_redis_worker(
    client: &redis::Client,
    jobs_by_name: Arc<RwLock<HashMap<String, JobInfo>>>,
    state: AppState,
    shutdown: tokio_util::sync::CancellationToken,
    worker_config: RedisWorkerConfig,
) -> Result<(), AutumnError> {
    let mut connection =
        new_redis_connection_manager(client, "jobs redis worker connection manager")?;

    tokio::spawn(async move {
        let mut retry_promotion_throttle = RedisMaintenanceThrottle::new(
            std::time::Instant::now(),
            worker_config.retry_promotion_interval,
        );
        let mut stale_recovery_throttle = RedisMaintenanceThrottle::new(
            std::time::Instant::now(),
            REDIS_STALE_MAINTENANCE_INTERVAL,
        );
        let idle_sleep = redis_worker_idle_sleep(worker_config.retry_promotion_interval);

        loop {
            if shutdown.is_cancelled() {
                break;
            }

            if retry_promotion_throttle.take_due(std::time::Instant::now()) {
                match promote_due_redis_retries(&mut connection, &worker_config, &state).await {
                    Ok(()) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, "redis job worker retry promotion failed");
                    }
                }
            }

            if stale_recovery_throttle.take_due(std::time::Instant::now()) {
                match recover_stale_redis_jobs(&mut connection, &worker_config, &state).await {
                    Ok(()) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, "redis job worker stale recovery failed");
                    }
                }
            }

            let Some(record) = (match claim_next_redis_job(&mut connection, &worker_config).await {
                Ok(record) => record,
                Err(error) => {
                    tracing::warn!(error = %error, "redis job worker claim failed");
                    tokio::time::sleep(idle_sleep).await;
                    continue;
                }
            }) else {
                tokio::time::sleep(idle_sleep).await;
                continue;
            };

            process_redis_job_record(
                &mut connection,
                record,
                &jobs_by_name,
                &state,
                &worker_config,
            )
            .await;
        }
    });

    Ok(())
}

#[cfg(feature = "redis")]
async fn settle_failed_redis_job(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    record: &RedisJobRecord,
    error: String,
    outcome: &str,
) {
    let action = prepare_redis_failure_action(record.clone(), error.clone(), now_unix_ms());
    match action {
        RedisFailureAction::Retry(schedule) => {
            match schedule_redis_retry(connection, worker_config, record, &schedule).await {
                Ok(true) => {
                    state
                        .job_registry
                        .record_retry(&schedule.record.name, &error, record.attempt);
                }
                Ok(false) => tracing::warn!(
                    job = %record.name,
                    job_id = %record.id,
                    outcome = %outcome,
                    "redis job retry skipped because claim changed"
                ),
                Err(error) => tracing::warn!(
                    job = %record.name,
                    job_id = %record.id,
                    outcome = %outcome,
                    error = %error,
                    "redis job retry scheduling failed"
                ),
            }
        }
        RedisFailureAction::DeadLetter(dead) => {
            match dead_letter_redis_job(connection, worker_config, record, &dead).await {
                Ok(true) => state.job_registry.record_failure(&dead.name, error, true),
                Ok(false) => tracing::warn!(
                    job = %record.name,
                    job_id = %record.id,
                    outcome = %outcome,
                    "redis job dead-letter skipped because claim changed"
                ),
                Err(error) => tracing::warn!(
                    job = %record.name,
                    job_id = %record.id,
                    outcome = %outcome,
                    error = %error,
                    "redis job dead-letter failed"
                ),
            }
        }
    }
}

#[cfg(feature = "redis")]
async fn dead_letter_panicked_redis_job(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    record: &RedisJobRecord,
    error: String,
) {
    let dead = prepare_redis_panic_dead_letter(record.clone(), error.clone());
    match dead_letter_redis_job(connection, worker_config, record, &dead).await {
        Ok(true) => state.job_registry.record_failure(&dead.name, error, true),
        Ok(false) => tracing::warn!(
            job = %record.name,
            job_id = %record.id,
            "redis job panic dead-letter skipped because claim changed"
        ),
        Err(error) => tracing::warn!(
            job = %record.name,
            job_id = %record.id,
            error = %error,
            "redis job panic dead-letter failed"
        ),
    }
}

#[cfg(feature = "redis")]
async fn process_redis_job_record(
    connection: &mut redis::aio::ConnectionManager,
    mut record: RedisJobRecord,
    jobs_by_name: &Arc<RwLock<HashMap<String, JobInfo>>>,
    state: &AppState,
    worker_config: &RedisWorkerConfig,
) {
    state.job_registry.record_start(&record.name);

    let maybe_info = {
        let guard = jobs_by_name.read().expect("job registry lock poisoned");
        guard
            .get(&record.name)
            .map(|info| (info.handler, info.max_attempts, info.initial_backoff_ms))
    };
    let Some((handler, info_max_attempts, info_backoff_ms)) = maybe_info else {
        state
            .job_registry
            .record_failure(&record.name, "unknown job type".to_owned(), true);
        let mut dead = record.clone();
        clear_redis_claim(&mut dead);
        dead.last_error = Some("unknown job type".to_string());
        let _ = dead_letter_redis_job(connection, worker_config, &record, &dead).await;
        return;
    };

    let max_attempts = if record.max_attempts != 0 {
        record.max_attempts
    } else if info_max_attempts != 0 {
        info_max_attempts
    } else {
        worker_config.default_attempts
    };
    let backoff_ms = if record.initial_backoff_ms != 0 {
        record.initial_backoff_ms
    } else if info_backoff_ms != 0 {
        info_backoff_ms
    } else {
        worker_config.default_backoff
    };
    record.max_attempts = max_attempts;
    record.initial_backoff_ms = backoff_ms;

    if record.attempt == 0 {
        state.job_registry.record_failure(
            &record.name,
            "invalid job payload: attempt must be >= 1".to_owned(),
            true,
        );
        let mut dead = record.clone();
        clear_redis_claim(&mut dead);
        dead.last_error = Some("invalid job payload: attempt must be >= 1".to_string());
        let _ = dead_letter_redis_job(connection, worker_config, &record, &dead).await;
        return;
    }

    match run_job_handler(handler, state.clone(), record.payload.clone()).await {
        JobExecutionOutcome::Succeeded => {
            match ack_redis_success(connection, worker_config, &record).await {
                Ok(true) => state.job_registry.record_success(&record.name),
                Ok(false) => tracing::warn!(
                    job = %record.name,
                    job_id = %record.id,
                    "redis job success ack skipped because claim changed"
                ),
                Err(error) => tracing::warn!(
                    job = %record.name,
                    job_id = %record.id,
                    error = %error,
                    "redis job success ack failed"
                ),
            }
        }
        JobExecutionOutcome::Failed(error) => {
            settle_failed_redis_job(connection, worker_config, state, &record, error, "failed")
                .await;
        }
        JobExecutionOutcome::Panicked(error) => {
            tracing::error!(job = %record.name, error = %error, "redis job handler panicked");
            dead_letter_panicked_redis_job(connection, worker_config, state, &record, error).await;
        }
    }
}

#[cfg(feature = "redis")]
fn start_redis_runtime(
    jobs: Vec<JobInfo>,
    state: &AppState,
    shutdown: &tokio_util::sync::CancellationToken,
    config: &crate::config::JobConfig,
) -> Result<(), AutumnError> {
    let url = config
        .redis
        .url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| {
            AutumnError::internal_server_error(std::io::Error::other(
                "jobs.backend=redis requires jobs.redis.url",
            ))
        })?;

    let client = redis::Client::open(url).map_err(|e| {
        AutumnError::internal_server_error(std::io::Error::other(format!(
            "invalid jobs redis url: {e}"
        )))
    })?;
    let producer_connection =
        new_redis_connection_manager(&client, "jobs redis connection manager")?;

    let queue_key = format!("{}:queue", config.redis.key_prefix);
    let processing_key = format!("{}:processing", config.redis.key_prefix);
    let delayed_key = format!("{}:delayed", config.redis.key_prefix);
    let dead_key = format!("{}:dead", config.redis.key_prefix);
    let record_prefix = format!("{}:record:", config.redis.key_prefix);

    let per_job_defaults = build_per_job_defaults(&jobs);
    let retry_promotion_interval = std::time::Duration::from_millis(
        redis_retry_promotion_interval_ms(config.initial_backoff_ms, &jobs),
    );
    let jobs_by_name: Arc<RwLock<HashMap<String, JobInfo>>> = Arc::new(RwLock::new(
        jobs.into_iter().map(|j| (j.name.clone(), j)).collect(),
    ));

    {
        let guard = jobs_by_name.read().expect("job registry lock poisoned");
        for name in guard.keys() {
            state.job_registry.register(name);
        }
    }

    init_global_job_client(JobClient {
        local_sender: None,
        redis: Some(RedisClient {
            connection: producer_connection,
            queue_key: queue_key.clone(),
            record_prefix: record_prefix.clone(),
        }),
        registry: state.job_registry.clone(),
        default_max_attempts: config.max_attempts,
        default_initial_backoff_ms: config.initial_backoff_ms,
        per_job_defaults,
    });

    let worker_count = config.workers.max(1);
    for _ in 0..worker_count {
        spawn_redis_worker(
            &client,
            Arc::clone(&jobs_by_name),
            state.clone(),
            shutdown.clone(),
            RedisWorkerConfig {
                queue_key: queue_key.clone(),
                processing_key: processing_key.clone(),
                delayed_key: delayed_key.clone(),
                dead_key: dead_key.clone(),
                record_prefix: record_prefix.clone(),
                worker_id: format!("{}:{}", std::process::id(), uuid::Uuid::new_v4()),
                visibility_timeout_ms: config.redis.visibility_timeout_ms,
                default_attempts: config.max_attempts,
                default_backoff: config.initial_backoff_ms,
                retry_promotion_interval,
            },
        )?;
    }

    Ok(())
}

fn build_per_job_defaults(jobs: &[JobInfo]) -> HashMap<String, (u32, u64)> {
    jobs.iter()
        .map(|job| (job.name.clone(), (job.max_attempts, job.initial_backoff_ms)))
        .collect()
}

fn validate_unique_job_names(jobs: &[JobInfo]) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    for job in jobs {
        if !names.insert(job.name.clone()) {
            return Err(format!("duplicate job name '{}'", job.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    #[cfg(feature = "redis")]
    static REDIS_HANDLER_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn always_fail_handler(
        _state: AppState,
        _payload: Value,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
        Box::pin(async move {
            Err(AutumnError::internal_server_error(std::io::Error::other(
                "forced failure",
            )))
        })
    }

    #[cfg(feature = "redis")]
    fn redis_counting_success_handler(
        _state: AppState,
        _payload: Value,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
        Box::pin(async move {
            REDIS_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    #[cfg(feature = "redis")]
    fn redis_counting_failure_handler(
        _state: AppState,
        _payload: Value,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
        Box::pin(async move {
            REDIS_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst);
            Err(AutumnError::internal_server_error(std::io::Error::other(
                "redis forced failure",
            )))
        })
    }

    fn panicking_handler(
        _state: AppState,
        _payload: Value,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
        Box::pin(async move {
            panic!("forced panic");
        })
    }

    fn instantly_panicking_handler(
        _state: AppState,
        _payload: Value,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
        panic!("panic before future")
    }

    #[tokio::test]
    async fn run_job_handler_reports_immediate_panics() {
        let state = AppState::for_test().with_profile("dev");
        let outcome =
            run_job_handler(instantly_panicking_handler, state, serde_json::json!({})).await;
        assert_eq!(
            outcome,
            JobExecutionOutcome::Panicked("job handler panicked: panic before future".to_string())
        );
    }

    #[tokio::test]
    async fn local_enqueue_p99_is_under_5ms() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let state = AppState::for_test().with_profile("dev");
        let shutdown = tokio_util::sync::CancellationToken::new();
        start_local_runtime(
            vec![JobInfo {
                name: "noop".to_string(),
                max_attempts: 3,
                initial_backoff_ms: 10,
                handler: |_state, _payload| Box::pin(async move { Ok(()) }),
            }],
            &state,
            &shutdown,
            1,
            5,
            250,
        );

        let mut samples = Vec::new();
        for _ in 0..300 {
            let started = std::time::Instant::now();
            enqueue("noop", serde_json::json!({})).await.unwrap();
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        let p99 = samples[(samples.len() * 99) / 100];
        assert!(
            p99 < std::time::Duration::from_millis(5),
            "expected p99 enqueue latency < 5ms, got {p99:?}",
        );

        shutdown.cancel();
        clear_global_job_client();
    }

    #[tokio::test]
    async fn local_panicking_handler_records_terminal_failure_without_requeue() {
        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("panic");
        state.job_registry().record_enqueue("panic");

        let mut jobs = HashMap::new();
        jobs.insert(
            "panic".to_string(),
            JobInfo {
                name: "panic".to_string(),
                max_attempts: 3,
                initial_backoff_ms: 1,
                handler: panicking_handler,
            },
        );
        let jobs_by_name = Arc::new(RwLock::new(jobs));

        let (tx, mut rx) = mpsc::channel(1);
        execute_local_job(
            QueuedJob {
                name: "panic".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 3,
                initial_backoff_ms: 1,
            },
            &jobs_by_name,
            &tx,
            &state,
        )
        .await;

        assert!(timeout(Duration::from_millis(25), rx.recv()).await.is_err());

        let snapshot = state.job_registry().snapshot();
        let status = snapshot.get("panic").expect("job should be registered");
        assert_eq!(status.queued, 0);
        assert_eq!(status.in_flight, 0);
        assert_eq!(status.total_failures, 1);
        assert_eq!(status.dead_letters, 1);
        assert_eq!(
            status.last_error.as_deref(),
            Some("job handler panicked: forced panic")
        );
    }

    #[tokio::test]
    async fn local_retry_records_enqueue_before_requeue() {
        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("flaky");
        state.job_registry().record_enqueue("flaky");

        let mut jobs = HashMap::new();
        jobs.insert(
            "flaky".to_string(),
            JobInfo {
                name: "flaky".to_string(),
                max_attempts: 2,
                initial_backoff_ms: 1,
                handler: always_fail_handler,
            },
        );
        let jobs_by_name = Arc::new(RwLock::new(jobs));

        let (tx, mut rx) = mpsc::channel(1);
        execute_local_job(
            QueuedJob {
                name: "flaky".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 2,
                initial_backoff_ms: 1,
            },
            &jobs_by_name,
            &tx,
            &state,
        )
        .await;

        let retried = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("retry should be scheduled")
            .expect("retry payload should be sent");
        assert_eq!(retried.name, "flaky");
        assert_eq!(retried.attempt, 2);

        let snapshot = state.job_registry().snapshot();
        let status = snapshot.get("flaky").expect("job should be registered");
        assert_eq!(status.queued, 1);
        assert_eq!(status.in_flight, 0);
        assert_eq!(status.total_failures, 0);
        assert!(status.last_error.is_some());
    }

    #[tokio::test]
    async fn local_terminal_failure_does_not_requeue() {
        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("flaky");
        state.job_registry().record_enqueue("flaky");

        let mut jobs = HashMap::new();
        jobs.insert(
            "flaky".to_string(),
            JobInfo {
                name: "flaky".to_string(),
                max_attempts: 1,
                initial_backoff_ms: 1,
                handler: always_fail_handler,
            },
        );
        let jobs_by_name = Arc::new(RwLock::new(jobs));

        let (tx, mut rx) = mpsc::channel(1);
        execute_local_job(
            QueuedJob {
                name: "flaky".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 1,
                initial_backoff_ms: 1,
            },
            &jobs_by_name,
            &tx,
            &state,
        )
        .await;

        assert!(timeout(Duration::from_millis(25), rx.recv()).await.is_err());

        let snapshot = state.job_registry().snapshot();
        let status = snapshot.get("flaky").expect("job should be registered");
        assert_eq!(status.queued, 0);
        assert_eq!(status.in_flight, 0);
        assert_eq!(status.total_failures, 1);
        assert_eq!(status.dead_letters, 1);
        assert!(status.last_error.is_some());
    }

    #[cfg(feature = "redis")]
    fn redis_test_record(attempt: u32, max_attempts: u32) -> RedisJobRecord {
        RedisJobRecord {
            id: "job-1".to_string(),
            name: "send_email".to_string(),
            payload: serde_json::json!({ "user_id": 42 }),
            attempt,
            max_attempts,
            initial_backoff_ms: 250,
            claimed_by: None,
            claimed_at_ms: None,
            last_error: None,
        }
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_claim_metadata_records_worker_and_deadline() {
        let claimed = claim_redis_record(redis_test_record(1, 3), "worker-a", 10_000, 30_000);

        assert_eq!(claimed.deadline_ms, 40_000);
        assert_eq!(claimed.record.claimed_by.as_deref(), Some("worker-a"));
        assert_eq!(claimed.record.claimed_at_ms, Some(10_000));
        assert_eq!(claimed.record.attempt, 1);
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_maintenance_throttle_runs_immediately_then_waits_for_interval() {
        let start = std::time::Instant::now();
        let mut throttle = RedisMaintenanceThrottle::new(start, Duration::from_secs(1));

        assert!(throttle.take_due(start));
        assert!(!throttle.take_due(start + Duration::from_millis(999)));
        assert!(throttle.take_due(start + Duration::from_secs(1)));
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_retry_promotion_interval_uses_smallest_configured_backoff() {
        let jobs = vec![
            JobInfo {
                name: "slow".to_string(),
                max_attempts: 3,
                initial_backoff_ms: 250,
                handler: redis_counting_success_handler,
            },
            JobInfo {
                name: "fast".to_string(),
                max_attempts: 3,
                initial_backoff_ms: 25,
                handler: redis_counting_success_handler,
            },
        ];

        assert_eq!(redis_retry_promotion_interval_ms(250, &jobs), 25);
        assert_eq!(redis_retry_promotion_interval_ms(0, &[]), 1);
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_worker_idle_sleep_is_bounded_by_retry_promotion_interval() {
        assert_eq!(
            redis_worker_idle_sleep(Duration::from_millis(25)),
            Duration::from_millis(25)
        );
        assert_eq!(
            redis_worker_idle_sleep(Duration::from_millis(250)),
            Duration::from_millis(200)
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_failed_job_schedules_next_attempt_with_exponential_backoff() {
        let mut record = redis_test_record(2, 4);
        record.claimed_by = Some("worker-a".to_string());
        record.claimed_at_ms = Some(20_000);

        let action = prepare_redis_failure_action(record, "stripe timed out".to_string(), 50_000);

        let RedisFailureAction::Retry(schedule) = action else {
            panic!("second attempt below max should be scheduled for retry");
        };
        assert_eq!(schedule.due_at_ms, 50_500);
        assert_eq!(schedule.record.attempt, 3);
        assert_eq!(schedule.record.claimed_by, None);
        assert_eq!(schedule.record.claimed_at_ms, None);
        assert_eq!(
            schedule.record.last_error.as_deref(),
            Some("stripe timed out")
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_failed_job_dead_letters_after_max_attempts() {
        let mut record = redis_test_record(3, 3);
        record.claimed_by = Some("worker-a".to_string());
        record.claimed_at_ms = Some(20_000);

        let action = prepare_redis_failure_action(record, "permanent failure".to_string(), 50_000);

        let RedisFailureAction::DeadLetter(record) = action else {
            panic!("max attempt failure should dead-letter");
        };
        assert_eq!(record.attempt, 3);
        assert_eq!(record.claimed_by, None);
        assert_eq!(record.claimed_at_ms, None);
        assert_eq!(record.last_error.as_deref(), Some("permanent failure"));
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_panicking_job_dead_letters_without_retry_even_when_attempts_remain() {
        let mut record = redis_test_record(1, 3);
        record.claimed_by = Some("worker-a".to_string());
        record.claimed_at_ms = Some(20_000);

        let dead = prepare_redis_panic_dead_letter(record, "job handler panicked".to_string());

        assert_eq!(dead.attempt, 1);
        assert_eq!(dead.max_attempts, 3);
        assert_eq!(dead.claimed_by, None);
        assert_eq!(dead.claimed_at_ms, None);
        assert_eq!(dead.last_error.as_deref(), Some("job handler panicked"));
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_stale_claim_recovery_requeues_next_attempt() {
        let mut record = redis_test_record(1, 3);
        record.claimed_by = Some("worker-a".to_string());
        record.claimed_at_ms = Some(10_000);

        let action = recover_stale_redis_record(record, 45_000, 30_000)
            .expect("expired claim should be recovered");

        let RedisStaleRecovery::Requeue(record) = action else {
            panic!("stale nonterminal claim should requeue");
        };
        assert_eq!(record.attempt, 2);
        assert_eq!(record.claimed_by, None);
        assert_eq!(record.claimed_at_ms, None);
        assert!(
            record
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("visibility timeout expired")),
            "stale recovery should record a useful last_error"
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_stale_claim_recovery_dead_letters_exhausted_job() {
        let mut record = redis_test_record(1, 1);
        record.claimed_by = Some("worker-a".to_string());
        record.claimed_at_ms = Some(10_000);

        let action = recover_stale_redis_record(record, 45_000, 30_000)
            .expect("expired claim should be recovered");

        let RedisStaleRecovery::DeadLetter(record) = action else {
            panic!("stale terminal claim should dead-letter");
        };
        assert_eq!(record.attempt, 1);
        assert_eq!(record.claimed_by, None);
        assert_eq!(record.claimed_at_ms, None);
        assert!(
            record
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("visibility timeout expired")),
            "dead-lettered stale claims should retain the recovery reason"
        );
    }

    #[cfg(feature = "redis")]
    fn redis_test_worker_config(
        prefix: &str,
        worker_id: &str,
        visibility_timeout_ms: u64,
    ) -> RedisWorkerConfig {
        RedisWorkerConfig {
            queue_key: format!("{prefix}:queue"),
            processing_key: format!("{prefix}:processing"),
            delayed_key: format!("{prefix}:delayed"),
            dead_key: format!("{prefix}:dead"),
            record_prefix: format!("{prefix}:record:"),
            worker_id: worker_id.to_string(),
            visibility_timeout_ms,
            default_attempts: 3,
            default_backoff: 1,
            retry_promotion_interval: Duration::from_millis(1),
        }
    }

    #[cfg(feature = "redis")]
    async fn redis_test_client() -> (
        testcontainers::ContainerAsync<testcontainers_modules::redis::Redis>,
        redis::Client,
    ) {
        use testcontainers::runners::AsyncRunner as _;
        use testcontainers_modules::redis::Redis as RedisImage;

        let container = RedisImage::default().start().await.unwrap();
        let port = container.get_host_port_ipv4(6379).await.unwrap();
        let url = format!("redis://127.0.0.1:{port}");
        (container, redis::Client::open(url).unwrap())
    }

    #[cfg(feature = "redis")]
    fn redis_jobs_by_name(
        handler: JobHandler,
        max_attempts: u32,
    ) -> Arc<RwLock<HashMap<String, JobInfo>>> {
        Arc::new(RwLock::new(HashMap::from([(
            "send_email".to_string(),
            JobInfo {
                name: "send_email".to_string(),
                max_attempts,
                initial_backoff_ms: 1,
                handler,
            },
        )])))
    }

    #[cfg(feature = "redis")]
    async fn redis_enqueue_test_job(
        client: &redis::Client,
        worker_config: &RedisWorkerConfig,
        max_attempts: u32,
    ) {
        let connection = new_redis_connection_manager(client, "test redis producer").unwrap();
        let producer = RedisClient {
            connection,
            queue_key: worker_config.queue_key.clone(),
            record_prefix: worker_config.record_prefix.clone(),
        };
        producer
            .enqueue(
                "send_email",
                serde_json::json!({ "user_id": 42 }),
                max_attempts,
                1,
            )
            .await
            .unwrap();
    }

    #[cfg(feature = "redis")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires Docker (testcontainers)"]
    async fn redis_claim_ack_deletes_record_only_after_success() {
        use redis::AsyncCommands as _;

        REDIS_HANDLER_CALLS.store(0, Ordering::SeqCst);
        let (_container, client) = redis_test_client().await;
        let worker_config = redis_test_worker_config("autumn:test:ack", "worker-a", 30_000);
        redis_enqueue_test_job(&client, &worker_config, 2).await;

        let mut connection = new_redis_connection_manager(&client, "test redis worker").unwrap();
        let record = claim_next_redis_job(&mut connection, &worker_config)
            .await
            .unwrap()
            .expect("job should be claimed");
        let record_key = redis_record_key(&worker_config.record_prefix, &record.id);
        let processing_count: usize = connection
            .zcard(&worker_config.processing_key)
            .await
            .unwrap();
        assert_eq!(processing_count, 1);

        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("send_email");
        state.job_registry().record_enqueue("send_email");
        process_redis_job_record(
            &mut connection,
            record,
            &redis_jobs_by_name(redis_counting_success_handler, 2),
            &state,
            &worker_config,
        )
        .await;

        let exists: bool = connection.exists(record_key).await.unwrap();
        let processing_count: usize = connection
            .zcard(&worker_config.processing_key)
            .await
            .unwrap();
        let dead_count: usize = connection.llen(&worker_config.dead_key).await.unwrap();
        assert!(!exists, "successful ack should delete the durable record");
        assert_eq!(processing_count, 0);
        assert_eq!(dead_count, 0);
        assert_eq!(REDIS_HANDLER_CALLS.load(Ordering::SeqCst), 1);
        let status = state.job_registry().snapshot()["send_email"].clone();
        assert_eq!(status.queued, 0);
        assert_eq!(status.in_flight, 0);
        assert_eq!(status.total_successes, 1);
    }

    #[cfg(feature = "redis")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires Docker (testcontainers)"]
    async fn redis_failure_retries_with_backoff_then_dead_letters() {
        use redis::AsyncCommands as _;

        REDIS_HANDLER_CALLS.store(0, Ordering::SeqCst);
        let (_container, client) = redis_test_client().await;
        let worker_config = redis_test_worker_config("autumn:test:retry", "worker-a", 30_000);
        redis_enqueue_test_job(&client, &worker_config, 2).await;

        let mut connection = new_redis_connection_manager(&client, "test redis worker").unwrap();
        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("send_email");
        state.job_registry().record_enqueue("send_email");
        let jobs = redis_jobs_by_name(redis_counting_failure_handler, 2);

        let first = claim_next_redis_job(&mut connection, &worker_config)
            .await
            .unwrap()
            .expect("first attempt should be claimed");
        process_redis_job_record(&mut connection, first, &jobs, &state, &worker_config).await;
        let delayed_count: usize = connection.zcard(&worker_config.delayed_key).await.unwrap();
        let processing_count: usize = connection
            .zcard(&worker_config.processing_key)
            .await
            .unwrap();
        assert_eq!(delayed_count, 1);
        assert_eq!(processing_count, 0);

        tokio::time::sleep(Duration::from_millis(5)).await;
        promote_due_redis_retries(&mut connection, &worker_config, &state)
            .await
            .unwrap();
        let queued_count: usize = connection.llen(&worker_config.queue_key).await.unwrap();
        assert_eq!(queued_count, 1);

        let second = claim_next_redis_job(&mut connection, &worker_config)
            .await
            .unwrap()
            .expect("retry attempt should be claimed");
        assert_eq!(second.attempt, 2);
        process_redis_job_record(&mut connection, second, &jobs, &state, &worker_config).await;

        let dead_count: usize = connection.llen(&worker_config.dead_key).await.unwrap();
        let delayed_count: usize = connection.zcard(&worker_config.delayed_key).await.unwrap();
        assert_eq!(dead_count, 1);
        assert_eq!(delayed_count, 0);
        assert_eq!(REDIS_HANDLER_CALLS.load(Ordering::SeqCst), 2);
        let status = state.job_registry().snapshot()["send_email"].clone();
        assert_eq!(status.in_flight, 0);
        assert_eq!(status.total_failures, 1);
        assert_eq!(status.dead_letters, 1);
    }

    #[cfg(feature = "redis")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires Docker (testcontainers)"]
    async fn redis_panicking_handler_dead_letters_without_retry() {
        use redis::AsyncCommands as _;

        let (_container, client) = redis_test_client().await;
        let worker_config = redis_test_worker_config("autumn:test:panic", "worker-a", 30_000);
        redis_enqueue_test_job(&client, &worker_config, 3).await;

        let mut connection = new_redis_connection_manager(&client, "test redis worker").unwrap();
        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("send_email");
        state.job_registry().record_enqueue("send_email");

        let record = claim_next_redis_job(&mut connection, &worker_config)
            .await
            .unwrap()
            .expect("panicking job should be claimed");
        process_redis_job_record(
            &mut connection,
            record,
            &redis_jobs_by_name(panicking_handler, 3),
            &state,
            &worker_config,
        )
        .await;

        let queued_count: usize = connection.llen(&worker_config.queue_key).await.unwrap();
        let delayed_count: usize = connection.zcard(&worker_config.delayed_key).await.unwrap();
        let processing_count: usize = connection
            .zcard(&worker_config.processing_key)
            .await
            .unwrap();
        let dead_count: usize = connection.llen(&worker_config.dead_key).await.unwrap();
        assert_eq!(queued_count, 0);
        assert_eq!(delayed_count, 0);
        assert_eq!(processing_count, 0);
        assert_eq!(dead_count, 1);

        let status = state.job_registry().snapshot()["send_email"].clone();
        assert_eq!(status.in_flight, 0);
        assert_eq!(status.total_failures, 1);
        assert_eq!(status.dead_letters, 1);
    }

    #[cfg(feature = "redis")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires Docker (testcontainers)"]
    async fn redis_stale_claim_recovery_requeues_for_another_worker() {
        use redis::AsyncCommands as _;

        let (_container, client) = redis_test_client().await;
        let worker_a = redis_test_worker_config("autumn:test:stale", "worker-a", 1);
        let worker_b = redis_test_worker_config("autumn:test:stale", "worker-b", 30_000);
        redis_enqueue_test_job(&client, &worker_a, 3).await;

        let mut connection = new_redis_connection_manager(&client, "test redis worker").unwrap();
        let claimed = claim_next_redis_job(&mut connection, &worker_a)
            .await
            .unwrap()
            .expect("first worker should claim the job");
        assert_eq!(claimed.claimed_by.as_deref(), Some("worker-a"));
        assert_eq!(claimed.attempt, 1);

        tokio::time::sleep(Duration::from_millis(5)).await;
        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("send_email");
        recover_stale_redis_jobs(&mut connection, &worker_b, &state)
            .await
            .unwrap();

        let queued_count: usize = connection.llen(&worker_b.queue_key).await.unwrap();
        assert_eq!(queued_count, 1);
        let reclaimed = claim_next_redis_job(&mut connection, &worker_b)
            .await
            .unwrap()
            .expect("second worker should reclaim stale job");
        assert_eq!(reclaimed.claimed_by.as_deref(), Some("worker-b"));
        assert_eq!(reclaimed.attempt, 2);
        assert!(
            reclaimed
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("visibility timeout expired"))
        );
    }

    #[tokio::test]
    async fn enqueue_rejects_unregistered_job_name_before_queueing() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let state = AppState::for_test().with_profile("dev");
        let shutdown = tokio_util::sync::CancellationToken::new();
        start_local_runtime(
            vec![JobInfo {
                name: "known".to_string(),
                max_attempts: 3,
                initial_backoff_ms: 10,
                handler: |_state, _payload| Box::pin(async move { Ok(()) }),
            }],
            &state,
            &shutdown,
            1,
            5,
            250,
        );

        let error = enqueue("typoed-job", serde_json::json!({}))
            .await
            .expect_err("unknown job names should be rejected before queueing");
        assert!(
            error
                .to_string()
                .contains("job 'typoed-job' is not registered"),
            "unexpected error: {error}"
        );

        let snapshot = state.job_registry().snapshot();
        assert!(
            !snapshot.contains_key("typoed-job"),
            "unknown jobs must not be recorded as queued"
        );
        let known = snapshot
            .get("known")
            .expect("registered job should remain in the registry");
        assert_eq!(known.queued, 0);
        assert_eq!(known.in_flight, 0);

        shutdown.cancel();
        clear_global_job_client();
    }

    #[tokio::test]
    async fn start_runtime_rejects_duplicate_job_names() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let state = AppState::for_test().with_profile("dev");
        let shutdown = tokio_util::sync::CancellationToken::new();
        let error = start_runtime(
            vec![
                JobInfo {
                    name: "dupe".to_string(),
                    max_attempts: 1,
                    initial_backoff_ms: 1,
                    handler: |_state, _payload| Box::pin(async move { Ok(()) }),
                },
                JobInfo {
                    name: "dupe".to_string(),
                    max_attempts: 1,
                    initial_backoff_ms: 1,
                    handler: |_state, _payload| Box::pin(async move { Ok(()) }),
                },
            ],
            &state,
            &shutdown,
            &crate::config::JobConfig::default(),
        )
        .expect_err("duplicate job names should surface as init errors");

        assert!(
            error
                .to_string()
                .contains("invalid jobs configuration: duplicate job name 'dupe'"),
            "unexpected error: {error}"
        );
        assert!(global_job_client().is_none());
    }

    #[tokio::test]
    #[cfg(not(feature = "redis"))]
    async fn start_runtime_rejects_redis_backend_when_feature_disabled() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let state = AppState::for_test().with_profile("dev");
        let shutdown = tokio_util::sync::CancellationToken::new();
        let config = crate::config::JobConfig {
            backend: "redis".to_string(),
            ..Default::default()
        };

        let error = start_runtime(
            vec![JobInfo {
                name: "known".to_string(),
                max_attempts: 1,
                initial_backoff_ms: 1,
                handler: |_state, _payload| Box::pin(async move { Ok(()) }),
            }],
            &state,
            &shutdown,
            &config,
        )
        .expect_err("redis backend must fail without the redis feature");

        assert!(
            error
                .to_string()
                .contains("jobs.backend=redis requested but redis feature is disabled"),
            "unexpected error: {error}"
        );
        assert!(global_job_client().is_none());
    }

    #[tokio::test]
    #[cfg(feature = "redis")]
    async fn start_runtime_rejects_redis_backend_without_url() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let state = AppState::for_test().with_profile("dev");
        let shutdown = tokio_util::sync::CancellationToken::new();
        let config = crate::config::JobConfig {
            backend: "redis".to_string(),
            redis: crate::config::JobRedisConfig {
                url: None,
                ..Default::default()
            },
            ..Default::default()
        };

        let error = start_runtime(
            vec![JobInfo {
                name: "known".to_string(),
                max_attempts: 1,
                initial_backoff_ms: 1,
                handler: |_state, _payload| Box::pin(async move { Ok(()) }),
            }],
            &state,
            &shutdown,
            &config,
        )
        .expect_err("redis backend must fail when its url is missing");

        assert!(
            error
                .to_string()
                .contains("jobs.backend=redis requires jobs.redis.url"),
            "unexpected error: {error}"
        );
        assert!(global_job_client().is_none());
    }

    #[tokio::test]
    async fn clear_global_job_client_resets_client() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();
        assert!(global_job_client().is_none());

        init_global_job_client(JobClient {
            local_sender: None,
            #[cfg(feature = "redis")]
            redis: None,
            registry: crate::actuator::JobRegistry::new(),
            default_max_attempts: 3,
            default_initial_backoff_ms: 250,
            per_job_defaults: HashMap::new(),
        });
        assert!(global_job_client().is_some());

        clear_global_job_client();
        assert!(global_job_client().is_none());
    }
}
