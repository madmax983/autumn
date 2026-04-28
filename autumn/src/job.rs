//! On-demand background job infrastructure.
//!
//! Provides [`JobInfo`] metadata used by `#[job]` and `jobs![]`, plus local
//! and Redis-backed queue backends.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

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

#[cfg(feature = "redis")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableQueuedJob {
    name: String,
    payload: Value,
    attempt: u32,
    max_attempts: u32,
    initial_backoff_ms: u64,
}

static GLOBAL_JOB_CLIENT: OnceLock<RwLock<JobClient>> = OnceLock::new();

#[must_use]
pub fn global_job_client() -> Option<JobClient> {
    GLOBAL_JOB_CLIENT
        .get()
        .and_then(|lock| lock.read().ok().map(|guard| guard.clone()))
}

pub(crate) fn init_global_job_client(client: JobClient) {
    if let Some(lock) = GLOBAL_JOB_CLIENT.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = client;
        }
        return;
    }
    let _ = GLOBAL_JOB_CLIENT.set(RwLock::new(client));
}

/// Enqueue a job payload on the configured runtime backend.
///
/// # Errors
///
/// Returns an internal error when the jobs runtime is not initialized or when
/// the active backend rejects the enqueue operation.
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
    /// Returns an internal error when enqueueing fails in the active backend.
    pub async fn enqueue(&self, name: &str, payload: Value) -> AutumnResult<()> {
        self.registry.record_enqueue(name);
        let (job_max_attempts, job_backoff_ms) = self
            .per_job_defaults
            .get(name)
            .copied()
            .unwrap_or((self.default_max_attempts, self.default_initial_backoff_ms));

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
) {
    validate_unique_job_names(&jobs).unwrap_or_else(|error| {
        panic!("invalid jobs configuration: {error}");
    });

    match config.backend.as_str() {
        "local" => start_local_runtime(
            jobs,
            state,
            shutdown,
            config.workers,
            config.max_attempts,
            config.initial_backoff_ms,
        ),
        "redis" => {
            #[cfg(feature = "redis")]
            {
                if let Err(error) =
                    start_redis_runtime(jobs.clone(), state, shutdown.clone(), config)
                {
                    tracing::error!(error = %error, "failed to start redis jobs backend; falling back to local backend");
                    start_local_runtime(
                        jobs,
                        state,
                        shutdown,
                        config.workers,
                        config.max_attempts,
                        config.initial_backoff_ms,
                    );
                }
            }
            #[cfg(not(feature = "redis"))]
            {
                tracing::warn!(
                    "jobs.backend=redis requested but redis feature is disabled; falling back to local backend"
                );
                start_local_runtime(
                    jobs,
                    state,
                    shutdown,
                    config.workers,
                    config.max_attempts,
                    config.initial_backoff_ms,
                );
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

    match (handler)(state.clone(), job.payload.clone()).await {
        Ok(()) => state.job_registry.record_success(&job.name),
        Err(e) => {
            if job.attempt < max_attempts {
                state
                    .job_registry
                    .record_retry(&job.name, &e.to_string(), job.attempt);
                let sender = tx.clone();
                let name = job.name.clone();
                let payload = job.payload;
                let delay = backoff_ms.saturating_mul(2_u64.saturating_pow(job.attempt - 1));
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
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
                state
                    .job_registry
                    .record_failure(&job.name, e.to_string(), true);
            }
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
fn start_redis_runtime(
    jobs: Vec<JobInfo>,
    state: &AppState,
    shutdown: tokio_util::sync::CancellationToken,
    config: &crate::config::JobConfig,
) -> Result<(), AutumnError> {
    use redis::aio::ConnectionManagerConfig;

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
    let connection =
        redis::aio::ConnectionManager::new_lazy_with_config(client, ConnectionManagerConfig::new())
            .map_err(|e| {
                AutumnError::internal_server_error(std::io::Error::other(format!(
                    "failed to create jobs redis connection manager: {e}"
                )))
            })?;

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
            connection: connection.clone(),
            queue_key: queue_key.clone(),
        }),
        registry: state.job_registry.clone(),
        default_max_attempts: config.max_attempts,
        default_initial_backoff_ms: config.initial_backoff_ms,
        per_job_defaults,
    });

    let worker_count = config.workers.max(1);
    for _ in 0..worker_count {
        let state = state.clone();
        let jobs_by_name = Arc::clone(&jobs_by_name);
        let mut connection = connection.clone();
        let queue_key = queue_key.clone();
        let dead_key = dead_key.clone();
        let shutdown = shutdown.clone();
        let default_attempts = config.max_attempts;
        let default_backoff = config.initial_backoff_ms;

        tokio::spawn(async move {
            use redis::AsyncCommands as _;

            loop {
                if shutdown.is_cancelled() {
                    break;
                }

                let popped = match connection
                    .brpop::<_, Option<[String; 2]>>(&queue_key, 1.0)
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

                let parsed: DurableQueuedJob = match serde_json::from_str(&body) {
                    Ok(v) => v,
                    Err(error) => {
                        tracing::warn!(error = %error, "invalid durable job payload");
                        let malformed = serde_json::json!({
                            "error": error.to_string(),
                            "raw_payload": body,
                        });
                        if let Ok(encoded) = serde_json::to_string(&malformed) {
                            let _ = connection.lpush::<_, _, ()>(&dead_key, encoded).await;
                        }
                        continue;
                    }
                };

                state.job_registry.record_start(&parsed.name);

                let (handler, max_attempts, backoff_ms) = {
                    let guard = jobs_by_name.read().expect("job registry lock poisoned");
                    let Some(info) = guard.get(&parsed.name) else {
                        state.job_registry.record_failure(
                            &parsed.name,
                            "unknown job type".to_owned(),
                            true,
                        );
                        if let Ok(encoded) = serde_json::to_string(&parsed) {
                            let _ = connection.lpush::<_, _, ()>(&dead_key, encoded).await;
                        }
                        continue;
                    };

                    (
                        info.handler,
                        if parsed.max_attempts != 0 {
                            parsed.max_attempts
                        } else if info.max_attempts != 0 {
                            info.max_attempts
                        } else {
                            default_attempts
                        },
                        if parsed.initial_backoff_ms != 0 {
                            parsed.initial_backoff_ms
                        } else if info.initial_backoff_ms != 0 {
                            info.initial_backoff_ms
                        } else {
                            default_backoff
                        },
                    )
                };

                match (handler)(state.clone(), parsed.payload.clone()).await {
                    Ok(()) => state.job_registry.record_success(&parsed.name),
                    Err(error) => {
                        if parsed.attempt < max_attempts {
                            state.job_registry.record_retry(
                                &parsed.name,
                                &error.to_string(),
                                parsed.attempt,
                            );
                            let delay =
                                backoff_ms.saturating_mul(2_u64.saturating_pow(parsed.attempt - 1));
                            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                            let retry = DurableQueuedJob {
                                name: parsed.name,
                                payload: parsed.payload,
                                attempt: parsed.attempt + 1,
                                max_attempts,
                                initial_backoff_ms: backoff_ms,
                            };
                            if let Ok(encoded) = serde_json::to_string(&retry) {
                                let _ = connection.lpush::<_, _, ()>(&queue_key, encoded).await;
                            }
                        } else {
                            state.job_registry.record_failure(
                                &parsed.name,
                                error.to_string(),
                                true,
                            );
                            if let Ok(encoded) = serde_json::to_string(&parsed) {
                                let _ = connection.lpush::<_, _, ()>(&dead_key, encoded).await;
                            }
                        }
                    }
                }
            }
        });
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

    #[tokio::test]
    async fn local_enqueue_p99_is_under_5ms() {
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
    }
}
