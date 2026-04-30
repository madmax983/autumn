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
struct DurableQueuedJob {
    name: String,
    payload: Value,
    attempt: u32,
    max_attempts: u32,
    initial_backoff_ms: u64,
}

#[cfg(feature = "redis")]
#[derive(Clone)]
struct RedisWorkerConfig {
    queue_key: String,
    dead_key: String,
    default_attempts: u32,
    default_backoff: u64,
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
    validate_unique_job_names(&jobs).unwrap_or_else(|error| {
        panic!("invalid jobs configuration: {error}");
    });

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
        use redis::AsyncCommands as _;

        let mut connection = self.connection.clone();
        let msg = DurableQueuedJob {
            name: name.to_string(),
            payload,
            attempt: 1,
            max_attempts: default_max_attempts,
            initial_backoff_ms: default_initial_backoff_ms,
        };
        let encoded = serde_json::to_string(&msg).map_err(|e| {
            AutumnError::internal_server_error(std::io::Error::other(format!(
                "serialize durable job failed: {e}"
            )))
        })?;

        connection
            .lpush::<_, _, ()>(&self.queue_key, encoded)
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
        use redis::AsyncCommands as _;

        loop {
            if shutdown.is_cancelled() {
                break;
            }

            let popped = match connection
                .brpop::<_, Option<[String; 2]>>(&worker_config.queue_key, 1.0)
                .await
            {
                Ok(v) => v,
                Err(error) => {
                    tracing::warn!(error = %error, "redis job worker brpop failed");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    continue;
                }
            };

            let Some([_, body]) = popped else {
                continue;
            };

            process_redis_job_message(&mut connection, body, &jobs_by_name, &state, &worker_config)
                .await;
        }
    });

    Ok(())
}

#[cfg(feature = "redis")]
async fn process_redis_job_message(
    connection: &mut redis::aio::ConnectionManager,
    body: String,
    jobs_by_name: &Arc<RwLock<HashMap<String, JobInfo>>>,
    state: &AppState,
    worker_config: &RedisWorkerConfig,
) {
    let parsed: DurableQueuedJob = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(error) => {
            tracing::warn!(error = %error, "invalid durable job payload");
            let malformed = serde_json::json!({
                "error": error.to_string(),
                "raw_payload": body,
            });
            push_json_list_item(connection, &worker_config.dead_key, &malformed).await;
            return;
        }
    };

    state.job_registry.record_start(&parsed.name);

    let maybe_info = {
        let guard = jobs_by_name.read().expect("job registry lock poisoned");
        guard
            .get(&parsed.name)
            .map(|info| (info.handler, info.max_attempts, info.initial_backoff_ms))
    };
    let Some((handler, info_max_attempts, info_backoff_ms)) = maybe_info else {
        state
            .job_registry
            .record_failure(&parsed.name, "unknown job type".to_owned(), true);
        push_json_list_item(connection, &worker_config.dead_key, &parsed).await;
        return;
    };

    let max_attempts = if parsed.max_attempts != 0 {
        parsed.max_attempts
    } else if info_max_attempts != 0 {
        info_max_attempts
    } else {
        worker_config.default_attempts
    };
    let backoff_ms = if parsed.initial_backoff_ms != 0 {
        parsed.initial_backoff_ms
    } else if info_backoff_ms != 0 {
        info_backoff_ms
    } else {
        worker_config.default_backoff
    };

    if parsed.attempt == 0 {
        state.job_registry.record_failure(
            &parsed.name,
            "invalid job payload: attempt must be >= 1".to_owned(),
            true,
        );
        push_json_list_item(connection, &worker_config.dead_key, &parsed).await;
        return;
    }

    match run_job_handler(handler, state.clone(), parsed.payload.clone()).await {
        JobExecutionOutcome::Succeeded => {
            state.job_registry.record_success(&parsed.name);
        }
        JobExecutionOutcome::Failed(error) => {
            if parsed.attempt < max_attempts {
                state
                    .job_registry
                    .record_retry(&parsed.name, &error, parsed.attempt);
                let exponent = parsed.attempt.saturating_sub(1);
                let delay = backoff_ms.saturating_mul(2_u64.saturating_pow(exponent));
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                let retry = DurableQueuedJob {
                    name: parsed.name,
                    payload: parsed.payload,
                    attempt: parsed.attempt + 1,
                    max_attempts,
                    initial_backoff_ms: backoff_ms,
                };
                state.job_registry.record_enqueue(&retry.name);
                push_json_list_item(connection, &worker_config.queue_key, &retry).await;
            } else {
                state.job_registry.record_failure(&parsed.name, error, true);
                push_json_list_item(connection, &worker_config.dead_key, &parsed).await;
            }
        }
        JobExecutionOutcome::Panicked(error) => {
            tracing::error!(job = %parsed.name, error = %error, "redis job handler panicked");
            state.job_registry.record_failure(&parsed.name, error, true);
            push_json_list_item(connection, &worker_config.dead_key, &parsed).await;
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
    let dead_key = format!("{}:dead", config.redis.key_prefix);

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

    init_global_job_client(JobClient {
        local_sender: None,
        redis: Some(RedisClient {
            connection: producer_connection,
            queue_key: queue_key.clone(),
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
                dead_key: dead_key.clone(),
                default_attempts: config.max_attempts,
                default_backoff: config.initial_backoff_ms,
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
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

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
