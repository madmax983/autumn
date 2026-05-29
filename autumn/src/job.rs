//! On-demand background job infrastructure.
//!
//! Provides [`JobInfo`] metadata used by `#[job]` and `jobs![]`, plus local
//! and Redis-backed queue backends.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use futures::FutureExt as _;
#[cfg(feature = "redis")]
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::{AppState, AutumnError, AutumnResult};

/// The asynchronous function signature for a background job.
///
/// Handlers receive the full `AppState` and a JSON `Value` representing the job's payload.
pub type JobHandler =
    fn(AppState, Value) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>>;

const DEFAULT_JOB_ADMIN_HISTORY_LIMIT: usize = 1_000;
const DEFAULT_JOB_ADMIN_PER_PAGE: u64 = 25;

/// Metadata describing a registered background job.
#[derive(Clone)]
pub struct JobInfo {
    /// The unique identifier for this job type.
    pub name: String,
    /// Maximum number of times a failing job will be retried.
    pub max_attempts: u32,
    /// Base delay in milliseconds before the first retry (scales exponentially).
    pub initial_backoff_ms: u64,
    /// The async function that executes the job logic.
    pub handler: JobHandler,
}

/// The runtime client for interacting with the job queue.
///
/// Used to enqueue jobs to the active backend (local or Redis).
#[derive(Clone)]
pub struct JobClient {
    local_sender: Option<tokio::sync::mpsc::Sender<QueuedJob>>,
    #[cfg(feature = "redis")]
    redis: Option<RedisClient>,
    #[cfg(feature = "db")]
    pg_pool: Option<PgPool>,
    registry: crate::actuator::JobRegistry,
    job_admin: JobAdminMemoryBackend,
    default_max_attempts: u32,
    default_initial_backoff_ms: u64,
    per_job_defaults: HashMap<String, (u32, u64)>,
    pub interceptor: Option<Arc<dyn crate::interceptor::JobInterceptor>>,
}

#[derive(Debug)]
struct QueuedJob {
    id: String,
    name: String,
    payload: Value,
    attempt: u32,
    max_attempts: u32,
    initial_backoff_ms: u64,
    /// W3C `traceparent` serialized at enqueue time.  `None` when the
    /// `telemetry-otlp` feature is disabled or no active span was present.
    #[cfg(feature = "telemetry-otlp")]
    traceparent: Option<String>,
    /// W3C `tracestate` serialized at enqueue time.
    #[cfg(feature = "telemetry-otlp")]
    tracestate: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum JobExecutionOutcome {
    Succeeded,
    Failed(String),
    Panicked(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobAdminStartDecision {
    Started,
    Canceled,
    Missing,
    AlreadyTransitioned,
}

/// Boxed future returned by job-admin backends.
pub type JobAdminFuture<'a, T> = Pin<Box<dyn Future<Output = AutumnResult<T>> + Send + 'a>>;

/// Human-facing lifecycle status for a background job entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobAdminStatus {
    /// Waiting to be picked up by a worker.
    Enqueued,
    /// Currently executing in a worker.
    Running,
    /// Failed but already scheduled for an automatic retry.
    Retrying,
    /// Finished successfully.
    Completed,
    /// Finished with a terminal error.
    Failed,
    /// Removed from the failed set by an operator.
    Discarded,
    /// Canceled before it started.
    Canceled,
    /// Re-enqueued by an operator from a failed entry.
    Retried,
}

impl JobAdminStatus {
    /// Stable display string used by the admin UI.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Enqueued => "enqueued",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Discarded => "discarded",
            Self::Canceled => "canceled",
            Self::Retried => "retried",
        }
    }
}

/// A job row exposed to the admin dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct JobAdminRecord {
    /// Stable runtime id for this job attempt.
    pub id: String,
    /// Job kind/name from `#[job(name = "...")]`.
    pub name: String,
    /// Current lifecycle status.
    pub status: JobAdminStatus,
    /// Time the job entered the queue.
    pub enqueued_at: Option<String>,
    /// Time the job started running.
    pub started_at: Option<String>,
    /// Time the job finished, failed, or was operated on.
    pub finished_at: Option<String>,
    /// Current attempt number.
    pub attempt: u32,
    /// Maximum attempts configured for this job.
    pub max_attempts: u32,
    /// Last observed error, if any.
    pub last_error: Option<String>,
    /// Principal/user id extracted from common payload fields, if present.
    pub principal_id: Option<String>,
    /// Correlation/request id extracted from common payload fields, if present.
    pub correlation_id: Option<String>,
}

/// Paginated records for one job status group.
#[derive(Debug, Clone, Serialize)]
pub struct JobAdminPage {
    /// Records for the requested page, sorted newest-first.
    pub records: Vec<JobAdminRecord>,
    /// Total records matching this status/time window.
    pub total: u64,
    /// Current page number, 1-indexed.
    pub page: u64,
    /// Records per page.
    pub per_page: u64,
}

impl JobAdminPage {
    /// Construct a page from preselected records.
    #[must_use]
    pub const fn new(records: Vec<JobAdminRecord>, total: u64, page: u64, per_page: u64) -> Self {
        Self {
            records,
            total,
            page,
            per_page,
        }
    }

    /// Total page count for this status group.
    #[must_use]
    pub const fn total_pages(&self) -> u64 {
        if self.per_page == 0 {
            return 0;
        }
        self.total.div_ceil(self.per_page)
    }
}

/// Scheduled task summary shown alongside ad-hoc jobs.
#[derive(Debug, Clone, Serialize)]
pub struct JobScheduleSummary {
    /// Registered scheduled task name.
    pub name: String,
    /// Human-readable schedule expression.
    pub schedule: String,
    /// Next scheduled run time, if the scheduler backend can report it.
    pub next_run_at: Option<String>,
    /// Last run result/status, if any.
    pub last_run_status: Option<String>,
}

/// Complete dashboard snapshot for `/admin/jobs`.
#[derive(Debug, Clone, Serialize)]
pub struct JobAdminSnapshot {
    /// Enqueued jobs, newest-first.
    pub enqueued: JobAdminPage,
    /// Running jobs, newest-first.
    pub running: JobAdminPage,
    /// Completed jobs from the last 24 hours, newest-first.
    pub completed: JobAdminPage,
    /// Failed jobs from the last 7 days, newest-first.
    pub failed: JobAdminPage,
    /// Scheduled task summaries.
    pub schedules: Vec<JobScheduleSummary>,
    /// Maximum number of lifecycle entries retained by the default backend.
    pub bounded_history_limit: usize,
}

impl JobAdminSnapshot {
    /// Empty snapshot for apps that have not initialized a jobs runtime.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            enqueued: JobAdminPage::new(Vec::new(), 0, 1, DEFAULT_JOB_ADMIN_PER_PAGE),
            running: JobAdminPage::new(Vec::new(), 0, 1, DEFAULT_JOB_ADMIN_PER_PAGE),
            completed: JobAdminPage::new(Vec::new(), 0, 1, DEFAULT_JOB_ADMIN_PER_PAGE),
            failed: JobAdminPage::new(Vec::new(), 0, 1, DEFAULT_JOB_ADMIN_PER_PAGE),
            schedules: Vec::new(),
            bounded_history_limit: DEFAULT_JOB_ADMIN_HISTORY_LIMIT,
        }
    }
}

/// Per-list pagination for the job dashboard.
#[derive(Debug, Clone)]
pub struct JobAdminQuery {
    /// Page number for enqueued jobs.
    pub enqueued_page: u64,
    /// Page number for running jobs.
    pub running_page: u64,
    /// Page number for completed jobs.
    pub completed_page: u64,
    /// Page number for failed jobs.
    pub failed_page: u64,
    /// Shared page size for all lists.
    pub per_page: u64,
}

impl Default for JobAdminQuery {
    fn default() -> Self {
        Self {
            enqueued_page: 1,
            running_page: 1,
            completed_page: 1,
            failed_page: 1,
            per_page: DEFAULT_JOB_ADMIN_PER_PAGE,
        }
    }
}

/// Read/operate surface consumed by first-party and custom job dashboards.
///
/// The default implementation is process-local and bounded. Durable external
/// queues can install their own backend in [`AppState`] by inserting
/// [`JobAdminBackendEntry`].
pub trait JobAdminBackend: Send + Sync + 'static {
    /// Return the dashboard snapshot for the supplied pagination.
    fn snapshot(&self, query: JobAdminQuery) -> JobAdminFuture<'_, JobAdminSnapshot>;

    /// Retry a failed job using its original payload.
    fn retry(&self, id: &str) -> JobAdminFuture<'_, ()>;

    /// Discard a failed job so it no longer appears in the failed list.
    fn discard(&self, id: &str) -> JobAdminFuture<'_, ()>;

    /// Cancel an enqueued job that has not started.
    fn cancel(&self, id: &str) -> JobAdminFuture<'_, ()>;
}

/// Typed [`AppState`] extension carrying a job-admin backend.
#[derive(Clone)]
pub struct JobAdminBackendEntry(pub Arc<dyn JobAdminBackend>);

/// Resolve the active job-admin backend from application state.
#[must_use]
pub fn job_admin_backend(state: &AppState) -> Option<Arc<dyn JobAdminBackend>> {
    state
        .extension::<JobAdminBackendEntry>()
        .map(|entry| Arc::clone(&entry.0))
}

#[derive(Debug, Clone)]
struct JobAdminStoredRecord {
    id: String,
    name: String,
    payload: Value,
    status: JobAdminStatus,
    enqueued_at: Option<chrono::DateTime<chrono::Utc>>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    attempt: u32,
    max_attempts: u32,
    last_error: Option<String>,
    principal_id: Option<String>,
    correlation_id: Option<String>,
}

impl JobAdminStoredRecord {
    fn sort_time(&self) -> chrono::DateTime<chrono::Utc> {
        self.finished_at
            .or(self.started_at)
            .or(self.enqueued_at)
            .unwrap_or_else(chrono::Utc::now)
    }

    fn to_public(&self) -> JobAdminRecord {
        JobAdminRecord {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status,
            enqueued_at: self.enqueued_at.map(format_job_admin_time),
            started_at: self.started_at.map(format_job_admin_time),
            finished_at: self.finished_at.map(format_job_admin_time),
            attempt: self.attempt,
            max_attempts: self.max_attempts,
            last_error: self.last_error.clone(),
            principal_id: self.principal_id.clone(),
            correlation_id: self.correlation_id.clone(),
        }
    }
}

#[derive(Debug)]
struct JobAdminMemoryInner {
    records: HashMap<String, JobAdminStoredRecord>,
    order: VecDeque<String>,
    history_limit: usize,
}

/// Bounded process-local job dashboard backend used by the built-in runtime.
#[derive(Clone)]
pub struct JobAdminMemoryBackend {
    inner: Arc<RwLock<JobAdminMemoryInner>>,
}

impl JobAdminMemoryBackend {
    /// Create a backend retaining the default number of lifecycle entries.
    #[must_use]
    pub fn new() -> Self {
        Self::with_history_limit(DEFAULT_JOB_ADMIN_HISTORY_LIMIT)
    }

    /// Create a backend retaining at most `history_limit` finished entries.
    #[must_use]
    pub fn with_history_limit(history_limit: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(JobAdminMemoryInner {
                records: HashMap::new(),
                order: VecDeque::new(),
                history_limit: history_limit.max(1),
            })),
        }
    }

    fn record_enqueue(
        &self,
        id: String,
        name: &str,
        payload: Value,
        attempt: u32,
        max_attempts: u32,
    ) {
        let now = chrono::Utc::now();
        let (principal_id, correlation_id) = job_payload_identity(&payload);
        if let Ok(mut inner) = self.inner.write() {
            inner.order.push_back(id.clone());
            inner.records.insert(
                id.clone(),
                JobAdminStoredRecord {
                    id,
                    name: name.to_owned(),
                    payload,
                    status: JobAdminStatus::Enqueued,
                    enqueued_at: Some(now),
                    started_at: None,
                    finished_at: None,
                    attempt,
                    max_attempts,
                    last_error: None,
                    principal_id,
                    correlation_id,
                },
            );
            prune_job_admin_history(&mut inner);
        }
    }

    fn record_requeued(&self, id: &str, attempt: u32) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(record) = inner.records.get_mut(id)
        {
            record.status = JobAdminStatus::Enqueued;
            record.enqueued_at = Some(chrono::Utc::now());
            record.started_at = None;
            record.finished_at = None;
            record.attempt = attempt;
        }
    }

    fn try_record_start(&self, id: &str, attempt: u32) -> JobAdminStartDecision {
        let Ok(mut inner) = self.inner.write() else {
            return JobAdminStartDecision::Missing;
        };
        let Some(record) = inner.records.get_mut(id) else {
            return JobAdminStartDecision::Missing;
        };
        match record.status {
            JobAdminStatus::Enqueued => {
                record.status = JobAdminStatus::Running;
                record.started_at = Some(chrono::Utc::now());
                record.finished_at = None;
                record.attempt = attempt;
                JobAdminStartDecision::Started
            }
            JobAdminStatus::Canceled => JobAdminStartDecision::Canceled,
            _ => JobAdminStartDecision::AlreadyTransitioned,
        }
    }

    fn record_success(&self, id: &str) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(record) = inner.records.get_mut(id)
        {
            record.status = JobAdminStatus::Completed;
            record.finished_at = Some(chrono::Utc::now());
            record.last_error = None;
            prune_job_admin_history(&mut inner);
        }
    }

    fn record_retrying(&self, id: &str, error: &str) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(record) = inner.records.get_mut(id)
        {
            record.status = JobAdminStatus::Retrying;
            record.finished_at = Some(chrono::Utc::now());
            record.last_error = Some(error.to_owned());
        }
    }

    fn record_failure(&self, id: &str, error: String) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(record) = inner.records.get_mut(id)
        {
            record.status = JobAdminStatus::Failed;
            record.finished_at = Some(chrono::Utc::now());
            record.last_error = Some(error);
            prune_job_admin_history(&mut inner);
        }
    }

    fn record_cancelled(&self, id: &str) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(record) = inner.records.get_mut(id)
        {
            record.status = JobAdminStatus::Canceled;
            record.finished_at = Some(chrono::Utc::now());
        }
    }

    fn retry_payload(&self, id: &str) -> AutumnResult<(String, Value)> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| AutumnError::internal_server_error_msg("job admin store lock poisoned"))?;
        let record = inner
            .records
            .get_mut(id)
            .ok_or_else(|| AutumnError::not_found_msg(format!("job '{id}' not found")))?;
        if record.status != JobAdminStatus::Failed {
            return Err(AutumnError::bad_request_msg(
                "only failed jobs can be retried",
            ));
        }
        let retry = (record.name.clone(), record.payload.clone());
        record.status = JobAdminStatus::Retried;
        record.finished_at = Some(chrono::Utc::now());
        drop(inner);
        Ok(retry)
    }

    fn restore_failed_retry(&self, id: &str) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(record) = inner.records.get_mut(id)
            && record.status == JobAdminStatus::Retried
        {
            record.status = JobAdminStatus::Failed;
            record.finished_at = Some(chrono::Utc::now());
        }
    }

    fn ensure_retryable(&self, id: &str) -> AutumnResult<()> {
        let inner = self
            .inner
            .read()
            .map_err(|_| AutumnError::internal_server_error_msg("job admin store lock poisoned"))?;
        let record = inner
            .records
            .get(id)
            .ok_or_else(|| AutumnError::not_found_msg(format!("job '{id}' not found")))?;
        let status = record.status;
        drop(inner);
        if status != JobAdminStatus::Failed {
            return Err(AutumnError::bad_request_msg(
                "only failed jobs can be retried",
            ));
        }
        Ok(())
    }

    fn discard_failed(&self, id: &str) -> AutumnResult<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| AutumnError::internal_server_error_msg("job admin store lock poisoned"))?;
        let record = inner
            .records
            .get_mut(id)
            .ok_or_else(|| AutumnError::not_found_msg(format!("job '{id}' not found")))?;
        if record.status != JobAdminStatus::Failed {
            return Err(AutumnError::bad_request_msg(
                "only failed jobs can be discarded",
            ));
        }
        record.status = JobAdminStatus::Discarded;
        record.finished_at = Some(chrono::Utc::now());
        drop(inner);
        Ok(())
    }

    fn cancel_enqueued(&self, id: &str) -> AutumnResult<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| AutumnError::internal_server_error_msg("job admin store lock poisoned"))?;
        let record = inner
            .records
            .get_mut(id)
            .ok_or_else(|| AutumnError::not_found_msg(format!("job '{id}' not found")))?;
        if record.status != JobAdminStatus::Enqueued {
            return Err(AutumnError::bad_request_msg(
                "only enqueued jobs can be canceled",
            ));
        }
        record.status = JobAdminStatus::Canceled;
        record.finished_at = Some(chrono::Utc::now());
        drop(inner);
        Ok(())
    }

    fn snapshot_sync(&self, query: &JobAdminQuery) -> JobAdminSnapshot {
        let Ok(inner) = self.inner.read() else {
            return JobAdminSnapshot::empty();
        };
        let now = chrono::Utc::now();
        let per_page = query.per_page.clamp(1, 100);
        JobAdminSnapshot {
            enqueued: paginate_job_admin_records(
                &inner,
                JobAdminStatus::Enqueued,
                None,
                query.enqueued_page,
                per_page,
            ),
            running: paginate_job_admin_records(
                &inner,
                JobAdminStatus::Running,
                None,
                query.running_page,
                per_page,
            ),
            completed: paginate_job_admin_records(
                &inner,
                JobAdminStatus::Completed,
                Some(now - chrono::TimeDelta::hours(24)),
                query.completed_page,
                per_page,
            ),
            failed: paginate_job_admin_records(
                &inner,
                JobAdminStatus::Failed,
                Some(now - chrono::TimeDelta::days(7)),
                query.failed_page,
                per_page,
            ),
            schedules: Vec::new(),
            bounded_history_limit: inner.history_limit,
        }
    }

    #[cfg(test)]
    fn new_for_test(history_limit: usize) -> Self {
        Self::with_history_limit(history_limit)
    }

    #[cfg(test)]
    fn record_enqueue_for_test(
        &self,
        name: &str,
        payload: Value,
        attempt: u32,
        max_attempts: u32,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.record_enqueue(id.clone(), name, payload, attempt, max_attempts);
        id
    }

    #[cfg(test)]
    fn record_start_for_test(&self, id: &str, attempt: u32) {
        let _ = self.try_record_start(id, attempt);
    }

    #[cfg(test)]
    fn record_success_for_test(&self, id: &str) {
        self.record_success(id);
    }

    #[cfg(test)]
    fn record_failure_for_test(&self, id: &str, error: &str) {
        self.record_failure(id, error.to_owned());
    }
}

impl Default for JobAdminMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl JobAdminBackend for JobAdminMemoryBackend {
    fn snapshot(&self, query: JobAdminQuery) -> JobAdminFuture<'_, JobAdminSnapshot> {
        Box::pin(async move { Ok(self.snapshot_sync(&query)) })
    }

    fn retry(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move {
            self.ensure_retryable(&id)?;
            let client = global_job_client().ok_or_else(|| {
                AutumnError::service_unavailable_msg("job runtime is not initialized")
            })?;
            let (name, payload) = self.retry_payload(&id)?;
            match client.enqueue(&name, payload).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.restore_failed_retry(&id);
                    Err(error)
                }
            }
        })
    }

    fn discard(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.discard_failed(&id) })
    }

    fn cancel(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.cancel_enqueued(&id) })
    }
}

fn format_job_admin_time(time: chrono::DateTime<chrono::Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn prune_job_admin_history(inner: &mut JobAdminMemoryInner) {
    let mut scanned = 0;
    while inner.order.len() > inner.history_limit && scanned < inner.order.len() {
        let Some(id) = inner.order.pop_front() else {
            break;
        };
        let is_active = inner.records.get(&id).is_some_and(|record| {
            matches!(
                record.status,
                JobAdminStatus::Enqueued | JobAdminStatus::Running | JobAdminStatus::Retrying
            )
        });
        if is_active {
            inner.order.push_back(id);
            scanned += 1;
        } else {
            inner.records.remove(&id);
        }
    }
}

fn paginate_job_admin_records(
    inner: &JobAdminMemoryInner,
    status: JobAdminStatus,
    since: Option<chrono::DateTime<chrono::Utc>>,
    page: u64,
    per_page: u64,
) -> JobAdminPage {
    let page = page.max(1);
    let mut records: Vec<_> = inner
        .records
        .values()
        .filter(|record| {
            record.status == status
                && since.is_none_or(|cutoff| {
                    record
                        .finished_at
                        .or(record.started_at)
                        .or(record.enqueued_at)
                        .is_some_and(|time| time >= cutoff)
                })
        })
        .cloned()
        .collect();
    records.sort_by_key(JobAdminStoredRecord::sort_time);
    records.reverse();

    let total = records.len() as u64;
    let start =
        usize::try_from(page.saturating_sub(1).saturating_mul(per_page)).unwrap_or(usize::MAX);
    let take = usize::try_from(per_page).unwrap_or(usize::MAX);
    let page_records = records
        .into_iter()
        .skip(start)
        .take(take)
        .map(|record| record.to_public())
        .collect();

    JobAdminPage::new(page_records, total, page, per_page)
}

fn job_payload_identity(payload: &Value) -> (Option<String>, Option<String>) {
    let principal = first_payload_string(payload, &["principal_id", "principal", "user_id"]);
    let correlation = first_payload_string(payload, &["correlation_id", "request_id"]);
    (principal, correlation)
}

fn first_payload_string(payload: &Value, keys: &[&str]) -> Option<String> {
    let object = payload.as_object()?;
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        if let Some(raw) = value.as_str() {
            if !raw.is_empty() {
                return Some(raw.to_owned());
            }
        } else if value.is_number() || value.is_boolean() {
            return Some(value.to_string());
        }
    }
    None
}

fn default_job_admin_backend_for_state(state: &AppState) -> JobAdminMemoryBackend {
    let backend = JobAdminMemoryBackend::new();
    if job_admin_backend(state).is_none() {
        state.insert_extension(JobAdminBackendEntry(Arc::new(backend.clone())));
    }
    backend
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
    enqueued_at_ms: Option<u64>,
    #[serde(default)]
    started_at_ms: Option<u64>,
    #[serde(default)]
    finished_at_ms: Option<u64>,
    #[serde(default)]
    claimed_by: Option<String>,
    #[serde(default)]
    claimed_at_ms: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
    /// W3C `traceparent` captured when the job was enqueued.
    #[cfg(feature = "telemetry-otlp")]
    #[serde(default)]
    traceparent: Option<String>,
    /// W3C `tracestate` captured when the job was enqueued.
    #[cfg(feature = "telemetry-otlp")]
    #[serde(default)]
    tracestate: Option<String>,
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
    completed_key: String,
    record_prefix: String,
    dead_record_prefix: String,
    worker_id: String,
    visibility_timeout_ms: u64,
    default_attempts: u32,
    default_backoff: u64,
    retry_promotion_interval: std::time::Duration,
}

static GLOBAL_JOB_CLIENT: OnceLock<RwLock<Option<Arc<JobClient>>>> = OnceLock::new();

pub fn global_job_runtime_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

// ── W3C Trace Context helpers ────────────────────────────────────────────────
//
// These are compiled only when `telemetry-otlp` is enabled.  The inject/
// extract helpers use a plain `HashMap` as the carrier so no HTTP crate is
// required here.

/// Serialize the current active span's W3C trace context into portable
/// strings `(traceparent, tracestate)`.  Returns `(None, None)` when no
/// global propagator is installed or no active span exists.
#[cfg(feature = "telemetry-otlp")]
fn capture_job_trace_context() -> (Option<String>, Option<String>) {
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;

    let cx = tracing::Span::current().context();
    let mut map = std::collections::HashMap::<String, String>::new();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut JobMapInjector(&mut map));
    });
    (map.remove("traceparent"), map.remove("tracestate"))
}

/// Reconstruct an OpenTelemetry [`Context`](opentelemetry::Context) from
/// serialized W3C `traceparent` / `tracestate` strings captured at enqueue
/// time.  Returns `None` when the `traceparent` is absent or unparseable so
/// the caller can fall back to a fresh root span instead of propagating a
/// broken context.
#[cfg(feature = "telemetry-otlp")]
fn restore_job_trace_context(
    traceparent: Option<&str>,
    tracestate: Option<&str>,
) -> Option<opentelemetry::Context> {
    use opentelemetry::trace::TraceContextExt as _;

    let tp = traceparent?;
    let mut map = std::collections::HashMap::<String, String>::new();
    map.insert("traceparent".to_owned(), tp.to_owned());
    if let Some(ts) = tracestate {
        map.insert("tracestate".to_owned(), ts.to_owned());
    }
    let cx = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&JobMapExtractor(&map))
    });
    if cx.span().span_context().is_valid() {
        Some(cx)
    } else {
        None
    }
}

#[cfg(feature = "telemetry-otlp")]
struct JobMapExtractor<'a>(&'a std::collections::HashMap<String, String>);

#[cfg(feature = "telemetry-otlp")]
impl opentelemetry::propagation::Extractor for JobMapExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

#[cfg(feature = "telemetry-otlp")]
struct JobMapInjector<'a>(&'a mut std::collections::HashMap<String, String>);

#[cfg(feature = "telemetry-otlp")]
impl opentelemetry::propagation::Injector for JobMapInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
}

fn build_job_consumer_span(name: &str, attempt: u32) -> tracing::Span {
    tracing::info_span!("job.execute", "otel.kind" = "consumer", job.name = %name, job.attempt = attempt)
}

async fn run_job_handler(
    name: &str,
    handler: JobHandler,
    state: AppState,
    payload: Value,
) -> JobExecutionOutcome {
    let interceptor = state
        .extension::<Arc<dyn crate::interceptor::JobInterceptor>>()
        .map(|arc| (*arc).clone());

    let payload_for_handler = payload.clone();
    // Defer the handler invocation into a lazy Pin<Box<dyn Future>>
    let next = Box::pin(async move {
        let future_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (handler)(state, payload_for_handler)
        }));

        let future = match future_res {
            Ok(f) => f,
            Err(panic) => {
                std::panic::resume_unwind(panic);
            }
        };

        future.await
    });

    let interceptor_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(interceptor) = &interceptor {
            interceptor.intercept_execute(name, &payload, next)
        } else {
            next
        }
    }));

    let future = match interceptor_res {
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

fn format_enqueue_panic(panic: &(dyn std::any::Any + Send)) -> AutumnError {
    let detail = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&'static str>().copied())
        .unwrap_or("non-string panic payload");
    AutumnError::internal_server_error(std::io::Error::other(format!(
        "job enqueue panicked: {detail}"
    )))
}

async fn run_enqueue_interceptor(
    interceptor: Arc<dyn crate::interceptor::JobInterceptor>,
    name: &str,
    payload: &Value,
    actual_enqueue: std::pin::Pin<
        Box<dyn std::future::Future<Output = AutumnResult<()>> + Send + '_>,
    >,
) -> AutumnResult<()> {
    let setup_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        interceptor.intercept_enqueue(name, payload, actual_enqueue)
    }));
    let fut = match setup_res {
        Ok(f) => f,
        Err(panic) => return Err(format_enqueue_panic(panic.as_ref())),
    };
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(res) => res,
        Err(panic) => Err(format_enqueue_panic(panic.as_ref())),
    }
}

/// Retrieves the global initialized job client.
///
/// Returns `None` if the job runtime hasn't been started yet.
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

pub fn clear_global_job_client() {
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

/// Enqueue a job using an **already-open connection** so the INSERT
/// participates in the caller's transaction.
///
/// For the `postgres` backend this provides atomic enqueue: if the
/// surrounding `db.tx` rolls back, the job disappears with it. For
/// `redis` and `local` backends the `conn` argument is ignored and the
/// call falls back to the normal enqueue path.
///
/// # Errors
///
/// Returns an error if the job runtime is not initialized, if `args`
/// cannot be serialized to JSON, or if the database INSERT fails.
///
/// # Example
///
/// ```rust,ignore
/// db.tx(move |conn| async move {
///     diesel::insert_into(orders::table).values(&order).execute(conn).await?;
///     autumn_web::job::enqueue_on_conn("send_confirmation", &args, conn).await?;
///     Ok(())
/// }.scope_boxed()).await?;
/// ```
#[cfg(feature = "db")]
pub async fn enqueue_on_conn<A: serde::Serialize>(
    name: &str,
    args: A,
    conn: &mut diesel_async::AsyncPgConnection,
) -> AutumnResult<()> {
    let payload = serde_json::to_value(&args).map_err(|e| {
        AutumnError::internal_server_error(std::io::Error::other(format!(
            "job args serialization failed: {e}"
        )))
    })?;
    let Some(client) = global_job_client() else {
        return Err(AutumnError::internal_server_error(std::io::Error::other(
            "job runtime is not initialized; register jobs with AppBuilder::jobs()",
        )));
    };
    client.enqueue_on_conn(name, payload, conn).await
}

/// Enqueue a job that fires **only after the surrounding transaction commits**.
///
/// This is the module-level companion to [`JobClient::enqueue_after_commit`].
/// It delegates to the globally initialized job client.
///
/// When called inside a [`Db::tx`](crate::db::Db::tx) block, the enqueue is
/// deferred until the transaction commits. On rollback the job is dropped.
/// This process-local deferral is not crash-safe: if the process exits after
/// the commit but before the callback runs, no job may be recorded.
///
/// When called outside any active transaction, the job is enqueued
/// immediately with a `debug`-level log noting the eager path.
///
/// For the `postgres` backend, prefer [`enqueue_in_tx`] when you have the
/// connection available: writing the job row inside the same transaction
/// gives exactly-once enqueue with no after-commit indirection.
///
/// # Errors
///
/// Returns an error if the job runtime is not initialized or if `args`
/// cannot be serialized to JSON.
pub async fn enqueue_after_commit<A: serde::Serialize>(name: &str, args: A) -> AutumnResult<()> {
    let payload = serde_json::to_value(&args).map_err(|e| {
        AutumnError::internal_server_error(std::io::Error::other(format!(
            "job args serialization failed: {e}"
        )))
    })?;
    let Some(client) = global_job_client() else {
        return Err(AutumnError::internal_server_error(std::io::Error::other(
            "job runtime is not initialized; register jobs with AppBuilder::jobs()",
        )));
    };
    client.enqueue_after_commit(name, payload).await
}

/// Enqueue a job inside an **already-open connection**, writing the job row
/// inside the caller's transaction for exactly-once semantics.
///
/// This is the optimal-path API for the `postgres` backend: the job row
/// is written inside the user's own DB transaction. If the transaction rolls
/// back, the job row disappears with it — no after-commit indirection needed.
///
/// For `redis` and `local` backends `conn` is ignored and the call falls back
/// to the normal enqueue path (same as [`enqueue_on_conn`]).
///
/// # Errors
///
/// Returns an error if the job runtime is not initialized, if `args`
/// cannot be serialized to JSON, or if the database INSERT fails.
///
/// # Example
///
/// ```rust,ignore
/// db.tx(move |conn| {
///     scoped_boxed(async move {
///         let user = diesel::insert_into(users::table).values(&new_user)
///             .get_result(conn).await?;
///         autumn_web::job::enqueue_in_tx("welcome_email", &WelcomeArgs { user_id: user.id }, conn).await?;
///         Ok(user)
///     })
/// }).await?;
/// ```
#[cfg(feature = "db")]
pub async fn enqueue_in_tx<A: serde::Serialize>(
    name: &str,
    args: A,
    conn: &mut diesel_async::AsyncPgConnection,
) -> AutumnResult<()> {
    enqueue_on_conn(name, args, conn).await
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
        let id = uuid::Uuid::new_v4().to_string();
        self.registry.record_enqueue(name);
        self.job_admin
            .record_enqueue(id.clone(), name, payload.clone(), 1, job_max_attempts);

        let started = ::std::sync::Arc::new(::std::sync::atomic::AtomicBool::new(false));
        let started_clone = started.clone();

        let id_for_enqueue = id.clone();
        let payload_clone = payload.clone();
        let actual_enqueue = async move {
            started_clone.store(true, ::std::sync::atomic::Ordering::SeqCst);
            let result = if let Some(sender) = &self.local_sender {
                #[cfg(feature = "telemetry-otlp")]
                let (traceparent, tracestate) = capture_job_trace_context();
                sender
                    .send(QueuedJob {
                        id: id_for_enqueue.clone(),
                        name: name.to_string(),
                        payload: payload_clone.clone(),
                        attempt: 1,
                        max_attempts: job_max_attempts,
                        initial_backoff_ms: job_backoff_ms,
                        #[cfg(feature = "telemetry-otlp")]
                        traceparent,
                        #[cfg(feature = "telemetry-otlp")]
                        tracestate,
                    })
                    .await
                    .map_err(|e| {
                        AutumnError::internal_server_error(std::io::Error::other(format!(
                            "failed to enqueue job: {e}"
                        )))
                    })
            } else {
                self.enqueue_durable(
                    id_for_enqueue.clone(),
                    name,
                    payload_clone.clone(),
                    job_max_attempts,
                    job_backoff_ms,
                )
                .await
            };
            if result.is_err() {
                self.registry.record_cancel(name);
                self.job_admin.record_cancelled(&id_for_enqueue);
            }
            result
        };

        let res = if let Some(interceptor) = &self.interceptor {
            let interceptor = (*interceptor).clone();
            run_enqueue_interceptor(interceptor, name, &payload, Box::pin(actual_enqueue)).await
        } else {
            actual_enqueue.await
        };

        if !started.load(::std::sync::atomic::Ordering::SeqCst) {
            self.registry.record_cancel(name);
            self.job_admin.record_cancelled(&id);
        }
        res
    }

    /// Enqueue a job that fires **only after the surrounding transaction commits**.
    ///
    /// When called inside a [`Db::tx`](crate::db::Db::tx) block, the enqueue is
    /// deferred until the transaction commits successfully. If the transaction
    /// rolls back, the job is never enqueued.
    ///
    /// The deferred enqueue callback runs in-process after commit. Use
    /// [`enqueue_in_tx`] / `enqueue_on_conn` with the
    /// Postgres backend when the job row itself must be committed atomically
    /// with the domain write.
    ///
    /// When called **outside** any active transaction, the job is enqueued
    /// immediately (equivalent to [`enqueue`](Self::enqueue)) and a `debug`-level
    /// log entry is emitted to make the no-op deferral visible.
    ///
    /// # Errors
    ///
    /// Returns an error if `payload` cannot be serialized to JSON, or if the
    /// underlying enqueue fails (backend error, unregistered job name, etc.).
    ///
    /// # Panics
    ///
    /// Panics if the internal after-commit registry mutex is poisoned (only
    /// possible if a previous thread holding the lock panicked, which should
    /// not occur in normal operation).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// db.tx(move |conn| scoped_boxed(async move {
    ///     let user = repo.create(new_user, conn).await?;
    ///     job_client
    ///         .enqueue_after_commit("welcome_email", WelcomeArgs { user_id: user.id })
    ///         .await?;
    ///     Ok(user)
    /// })).await?;
    /// ```
    pub async fn enqueue_after_commit(
        &self,
        name: &str,
        payload: impl serde::Serialize,
    ) -> AutumnResult<()> {
        // Validate name eagerly so a typo/unregistered job fails the
        // transaction (before any DB commit) rather than being silently
        // dropped later when the deferred callback runs.
        if !self.per_job_defaults.contains_key(name) {
            return Err(AutumnError::internal_server_error(std::io::Error::other(
                format!("job '{name}' is not registered; add it to AppBuilder::jobs()"),
            )));
        }

        let name = name.to_string();
        let payload = serde_json::to_value(payload).map_err(|e| {
            AutumnError::internal_server_error(std::io::Error::other(format!(
                "enqueue_after_commit: failed to serialize payload for job '{name}': {e}"
            )))
        })?;
        let client = self.clone();
        // Keep a copy for the debug log in the eager path (name is moved into f_opt).
        let name_for_log = name.clone();

        // Capture the caller's span now so that capture_job_trace_context() inside
        // client.enqueue() sees the originating request span even when the callback
        // runs in the after-commit task, which has no request span of its own.
        let enqueue_span = tracing::Span::current();

        let mut f_opt = Some(move || {
            let client = client.clone();
            let name = name.clone();
            let payload = payload.clone();
            async move { client.enqueue(&name, payload).await }
        });

        #[cfg(feature = "db")]
        crate::db::AFTER_COMMIT_REGISTRY
            .try_with(|registry| {
                let f = f_opt.take().expect("closure only entered once");
                let span = enqueue_span.clone();
                let boxed: crate::db::CommitCallback =
                    Box::new(move || Box::pin(tracing::Instrument::instrument(f(), span)));
                registry.lock().expect("registry lock").push(boxed);
            })
            .ok();

        if let Some(f) = f_opt {
            // Not inside a db.tx (or db feature is off) — enqueue immediately.
            tracing::debug!(
                "enqueue_after_commit: no active transaction; enqueueing '{name_for_log}' immediately"
            );
            f().await?;
        }

        Ok(())
    }

    async fn enqueue_durable(
        &self,
        id: String,
        name: &str,
        payload: Value,
        max_attempts: u32,
        backoff_ms: u64,
    ) -> AutumnResult<()> {
        #[cfg(feature = "redis")]
        if let Some(redis) = &self.redis {
            return redis
                .enqueue(id, name, payload, max_attempts, backoff_ms)
                .await;
        }
        #[cfg(feature = "db")]
        if let Some(pool) = &self.pg_pool {
            return pg_enqueue_job(pool, id, name, payload, max_attempts, backoff_ms).await;
        }
        let _ = (id, name, payload, max_attempts, backoff_ms);
        Err(AutumnError::internal_server_error(std::io::Error::other(
            "job runtime backend is unavailable",
        )))
    }

    /// Enqueue a job using an **already-open connection**, so the INSERT
    /// participates in the caller's transaction.
    ///
    /// For the `postgres` backend this provides exactly-once-per-commit
    /// enqueue semantics: if the surrounding `db.tx` rolls back, the job row
    /// disappears atomically. For `redis` and `local` backends the `conn`
    /// argument is ignored and the call falls back to the normal enqueue path.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a registered job, or if the
    /// database INSERT fails.
    #[cfg(feature = "db")]
    pub async fn enqueue_on_conn(
        &self,
        name: &str,
        payload: Value,
        conn: &mut diesel_async::AsyncPgConnection,
    ) -> AutumnResult<()> {
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
        let id = uuid::Uuid::new_v4().to_string();

        // Postgres transactional path: the caller controls when the surrounding
        // transaction commits, so we cannot safely update process-local counters
        // here — the row may disappear on rollback while the counter persists.
        if self.pg_pool.is_some() {
            let actual_enqueue = pg_enqueue_on_conn(
                conn,
                id,
                name,
                payload.clone(),
                job_max_attempts,
                job_backoff_ms,
            );
            return if let Some(interceptor) = &self.interceptor {
                let interceptor = (*interceptor).clone();
                run_enqueue_interceptor(interceptor, name, &payload, Box::pin(actual_enqueue)).await
            } else {
                actual_enqueue.await
            };
        }

        self.registry.record_enqueue(name);
        self.job_admin
            .record_enqueue(id.clone(), name, payload.clone(), 1, job_max_attempts);

        let started = ::std::sync::Arc::new(::std::sync::atomic::AtomicBool::new(false));
        let started_clone = started.clone();

        #[cfg(feature = "telemetry-otlp")]
        let (traceparent, tracestate) = capture_job_trace_context();

        let id_for_enqueue = id.clone();
        let payload_clone = payload.clone();
        let actual_enqueue = async move {
            started_clone.store(true, ::std::sync::atomic::Ordering::SeqCst);
            let result = if let Some(sender) = &self.local_sender {
                sender
                    .send(QueuedJob {
                        id: id_for_enqueue.clone(),
                        name: name.to_string(),
                        payload: payload_clone.clone(),
                        attempt: 1,
                        max_attempts: job_max_attempts,
                        initial_backoff_ms: job_backoff_ms,
                        #[cfg(feature = "telemetry-otlp")]
                        traceparent,
                        #[cfg(feature = "telemetry-otlp")]
                        tracestate,
                    })
                    .await
                    .map_err(|e| {
                        AutumnError::internal_server_error(std::io::Error::other(format!(
                            "failed to enqueue job: {e}"
                        )))
                    })
            } else {
                self.enqueue_durable(
                    id_for_enqueue.clone(),
                    name,
                    payload_clone.clone(),
                    job_max_attempts,
                    job_backoff_ms,
                )
                .await
            };
            if result.is_err() {
                self.registry.record_cancel(name);
                self.job_admin.record_cancelled(&id_for_enqueue);
            }
            result
        };

        let res = if let Some(interceptor) = &self.interceptor {
            let interceptor = (*interceptor).clone();
            run_enqueue_interceptor(interceptor, name, &payload, Box::pin(actual_enqueue)).await
        } else {
            actual_enqueue.await
        };

        if !started.load(::std::sync::atomic::Ordering::SeqCst) {
            self.registry.record_cancel(name);
            self.job_admin.record_cancelled(&id);
        }
        res
    }
}

/// Starts the background job execution runtime.
///
/// This initializes the configured job worker backend (local, redis, or postgres)
/// and launches background worker tasks that run until the shutdown cancellation token is triggered.
///
/// # Errors
///
/// Returns an error if:
/// - There are duplicate job names registered in the workspace
/// - Redis or Postgres connection/initialization fails (if those backends are selected)
pub fn start_runtime(
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
        "postgres" => {
            #[cfg(feature = "db")]
            {
                start_postgres_runtime(jobs, state, shutdown, config)
            }
            #[cfg(not(feature = "db"))]
            {
                let _ = (jobs, state, shutdown, config);
                Err(AutumnError::internal_server_error(std::io::Error::other(
                    "jobs.backend=postgres requested but db feature is disabled",
                )))
            }
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
    let job_admin = default_job_admin_backend_for_state(state);
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
        #[cfg(feature = "db")]
        pg_pool: None,
        registry: state.job_registry.clone(),
        job_admin: job_admin.clone(),
        default_max_attempts,
        default_initial_backoff_ms,
        per_job_defaults,
        interceptor: state
            .extension::<Arc<dyn crate::interceptor::JobInterceptor>>()
            .map(|arc| (*arc).clone()),
    };
    init_global_job_client(client);

    for _ in 0..worker_count {
        let state = state.clone();
        let tx = tx.clone();
        let job_admin = job_admin.clone();
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
                        execute_local_job(job, &jobs_by_name, &tx, &state, &job_admin).await;
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
    job_admin: &JobAdminMemoryBackend,
) {
    if job_admin.try_record_start(&job.id, job.attempt) == JobAdminStartDecision::Canceled {
        state.job_registry.record_cancel(&job.name);
        job_admin.record_cancelled(&job.id);
        return;
    }
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
        job_admin.record_failure(&job.id, format!("unknown job '{}'", job.name));
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

    let job_span = build_job_consumer_span(&job.name, job.attempt);
    #[cfg(feature = "telemetry-otlp")]
    if let Some(cx) =
        restore_job_trace_context(job.traceparent.as_deref(), job.tracestate.as_deref())
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        let _ = job_span.set_parent(cx);
    }
    let f = run_job_handler(&job.name, handler, state.clone(), job.payload.clone());
    let outcome = tracing::Instrument::instrument(f, job_span).await;
    match outcome {
        JobExecutionOutcome::Succeeded => {
            state.job_registry.record_success(&job.name);
            job_admin.record_success(&job.id);
        }
        JobExecutionOutcome::Failed(error) => {
            if job.attempt < max_attempts {
                state
                    .job_registry
                    .record_retry(&job.name, &error, job.attempt);
                job_admin.record_retrying(&job.id, &error);
                let sender = tx.clone();
                let registry = state.job_registry.clone();
                let job_admin = job_admin.clone();
                let id = job.id.clone();
                let name = job.name.clone();
                let payload = job.payload;
                #[cfg(feature = "telemetry-otlp")]
                let traceparent = job.traceparent;
                #[cfg(feature = "telemetry-otlp")]
                let tracestate = job.tracestate;
                let delay = backoff_ms.saturating_mul(2_u64.saturating_pow(job.attempt - 1));
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    registry.record_enqueue(&name);
                    job_admin.record_requeued(&id, job.attempt + 1);
                    let _ = sender
                        .send(QueuedJob {
                            id,
                            name,
                            payload,
                            attempt: job.attempt + 1,
                            max_attempts,
                            initial_backoff_ms: backoff_ms,
                            #[cfg(feature = "telemetry-otlp")]
                            traceparent,
                            #[cfg(feature = "telemetry-otlp")]
                            tracestate,
                        })
                        .await;
                });
            } else {
                state
                    .job_registry
                    .record_failure(&job.name, error.clone(), true);
                job_admin.record_failure(&job.id, error);
            }
        }
        JobExecutionOutcome::Panicked(error) => {
            tracing::error!(job = %job.name, error = %error, "local job handler panicked");
            state
                .job_registry
                .record_failure(&job.name, error.clone(), true);
            job_admin.record_failure(&job.id, error);
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
    record.finished_at_ms = Some(now_ms);

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
fn prepare_redis_panic_dead_letter(
    mut record: RedisJobRecord,
    error: String,
    now_ms: u64,
) -> RedisJobRecord {
    clear_redis_claim(&mut record);
    record.last_error = Some(error);
    record.finished_at_ms = Some(now_ms);
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
    record.finished_at_ms = Some(now_ms);
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
        id: String,
        name: &str,
        payload: Value,
        default_max_attempts: u32,
        default_initial_backoff_ms: u64,
    ) -> AutumnResult<()> {
        #[cfg(feature = "telemetry-otlp")]
        let (traceparent, tracestate) = capture_job_trace_context();
        let mut connection = self.connection.clone();
        let msg = RedisJobRecord {
            id: id.clone(),
            name: name.to_string(),
            payload,
            attempt: 1,
            max_attempts: default_max_attempts,
            initial_backoff_ms: default_initial_backoff_ms,
            enqueued_at_ms: Some(now_unix_ms()),
            started_at_ms: None,
            finished_at_ms: None,
            claimed_by: None,
            claimed_at_ms: None,
            last_error: None,
            #[cfg(feature = "telemetry-otlp")]
            traceparent,
            #[cfg(feature = "telemetry-otlp")]
            tracestate,
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
#[derive(Clone)]
struct RedisJobAdminBackend {
    connection: redis::aio::ConnectionManager,
    queue_key: String,
    processing_key: String,
    dead_key: String,
    completed_key: String,
    record_prefix: String,
    dead_record_prefix: String,
    history_limit: usize,
}

#[cfg(feature = "redis")]
impl RedisJobAdminBackend {
    #[allow(clippy::too_many_arguments)]
    fn new(
        connection: redis::aio::ConnectionManager,
        queue_key: String,
        processing_key: String,
        dead_key: String,
        completed_key: String,
        record_prefix: String,
        dead_record_prefix: String,
        history_limit: usize,
    ) -> Self {
        Self {
            connection,
            queue_key,
            processing_key,
            dead_key,
            completed_key,
            record_prefix,
            dead_record_prefix,
            history_limit: history_limit.max(1),
        }
    }

    async fn snapshot_redis(&self, query: &JobAdminQuery) -> AutumnResult<JobAdminSnapshot> {
        let mut connection = self.connection.clone();
        let per_page = query.per_page.clamp(1, 100);
        let now_ms = now_unix_ms();
        let completed_since = now_ms.saturating_sub(86_400_000);
        let failed_since = now_ms.saturating_sub(604_800_000);

        let enqueued = redis_admin_active_list_page(
            &mut connection,
            &self.queue_key,
            &self.record_prefix,
            JobAdminStatus::Enqueued,
            query.enqueued_page,
            per_page,
        )
        .await?;
        let running = redis_admin_running_page(
            &mut connection,
            &self.processing_key,
            &self.record_prefix,
            query.running_page,
            per_page,
        )
        .await?;
        let completed = redis_admin_encoded_list_page(
            &mut connection,
            &self.completed_key,
            JobAdminStatus::Completed,
            Some(completed_since),
            query.completed_page,
            per_page,
            self.history_limit,
        )
        .await?;
        let failed = redis_admin_encoded_list_page(
            &mut connection,
            &self.dead_key,
            JobAdminStatus::Failed,
            Some(failed_since),
            query.failed_page,
            per_page,
            self.history_limit,
        )
        .await?;

        Ok(JobAdminSnapshot {
            enqueued,
            running,
            completed,
            failed,
            schedules: Vec::new(),
            bounded_history_limit: self.history_limit,
        })
    }

    async fn retry_failed_redis(&self, id: &str) -> AutumnResult<()> {
        let mut connection = self.connection.clone();
        let new_id = uuid::Uuid::new_v4().to_string();
        let dead_record_key = format!("{}{id}", self.dead_record_prefix);
        let result: i64 = redis::cmd("EVAL")
            .arg(
                r"
local failed = redis.call('GET', KEYS[1])
if not failed then
  return 0
end
if redis.call('LREM', KEYS[2], 0, failed) == 0 then
  return -1
end
local ok, record = pcall(cjson.decode, failed)
if not ok then
  return -2
end
redis.call('DEL', KEYS[1])
record['id'] = ARGV[1]
record['attempt'] = 1
record['enqueued_at_ms'] = tonumber(ARGV[2])
record['started_at_ms'] = nil
record['finished_at_ms'] = nil
record['claimed_by'] = nil
record['claimed_at_ms'] = nil
record['last_error'] = nil
local active = cjson.encode(record)
redis.call('SET', KEYS[3] .. ARGV[1], active)
redis.call('LPUSH', KEYS[4], ARGV[1])
return 1
",
            )
            .arg(4)
            .arg(dead_record_key)
            .arg(&self.dead_key)
            .arg(&self.record_prefix)
            .arg(&self.queue_key)
            .arg(new_id)
            .arg(now_unix_ms())
            .query_async(&mut connection)
            .await
            .map_err(|error| redis_admin_error("retry failed job", &error))?;
        redis_admin_operation_result(result, id, "retry failed job")
    }

    async fn discard_failed_redis(&self, id: &str) -> AutumnResult<()> {
        let mut connection = self.connection.clone();
        let dead_record_key = format!("{}{id}", self.dead_record_prefix);
        let result: i64 = redis::cmd("EVAL")
            .arg(
                r"
local failed = redis.call('GET', KEYS[1])
if not failed then
  return 0
end
if redis.call('LREM', KEYS[2], 0, failed) == 0 then
  return -1
end
redis.call('DEL', KEYS[1])
return 1
",
            )
            .arg(2)
            .arg(dead_record_key)
            .arg(&self.dead_key)
            .query_async(&mut connection)
            .await
            .map_err(|error| redis_admin_error("discard failed job", &error))?;
        redis_admin_operation_result(result, id, "discard failed job")
    }

    async fn cancel_enqueued_redis(&self, id: &str) -> AutumnResult<()> {
        let mut connection = self.connection.clone();
        let active_record_key = redis_record_key(&self.record_prefix, id);
        let result: i64 = redis::cmd("EVAL")
            .arg(
                r"
if not redis.call('GET', KEYS[1]) then
  return 0
end
if redis.call('LREM', KEYS[2], 0, ARGV[1]) == 0 then
  return -1
end
redis.call('DEL', KEYS[1])
return 1
",
            )
            .arg(2)
            .arg(active_record_key)
            .arg(&self.queue_key)
            .arg(id)
            .query_async(&mut connection)
            .await
            .map_err(|error| redis_admin_error("cancel enqueued job", &error))?;
        redis_admin_operation_result(result, id, "cancel enqueued job")
    }
}

#[cfg(feature = "redis")]
impl JobAdminBackend for RedisJobAdminBackend {
    fn snapshot(&self, query: JobAdminQuery) -> JobAdminFuture<'_, JobAdminSnapshot> {
        Box::pin(async move { self.snapshot_redis(&query).await })
    }

    fn retry(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.retry_failed_redis(&id).await })
    }

    fn discard(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.discard_failed_redis(&id).await })
    }

    fn cancel(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.cancel_enqueued_redis(&id).await })
    }
}

#[cfg(feature = "redis")]
fn redis_admin_error(operation: &str, error: &redis::RedisError) -> AutumnError {
    AutumnError::internal_server_error(std::io::Error::other(format!(
        "redis job admin {operation} failed: {error}"
    )))
}

#[cfg(feature = "redis")]
fn redis_admin_operation_result(result: i64, id: &str, operation: &str) -> AutumnResult<()> {
    match result {
        1 => Ok(()),
        0 => Err(AutumnError::not_found_msg(format!("job '{id}' not found"))),
        -1 => Err(AutumnError::bad_request_msg(format!(
            "job '{id}' is not in the expected state for {operation}"
        ))),
        -2 => Err(AutumnError::internal_server_error_msg(format!(
            "job '{id}' has an invalid stored payload"
        ))),
        _ => Err(AutumnError::internal_server_error_msg(format!(
            "redis job admin {operation} returned unexpected code {result}"
        ))),
    }
}

#[cfg(feature = "redis")]
fn redis_admin_time(ms: Option<u64>) -> Option<String> {
    let ms = i64::try_from(ms?).ok()?;
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).map(format_job_admin_time)
}

#[cfg(feature = "redis")]
fn redis_record_sort_time(record: &RedisJobRecord) -> u64 {
    record
        .finished_at_ms
        .or(record.started_at_ms)
        .or(record.enqueued_at_ms)
        .unwrap_or_default()
}

#[cfg(feature = "redis")]
fn redis_record_to_admin_record(record: &RedisJobRecord, status: JobAdminStatus) -> JobAdminRecord {
    let (principal_id, correlation_id) = job_payload_identity(&record.payload);
    JobAdminRecord {
        id: record.id.clone(),
        name: record.name.clone(),
        status,
        enqueued_at: redis_admin_time(record.enqueued_at_ms),
        started_at: redis_admin_time(record.started_at_ms),
        finished_at: redis_admin_time(record.finished_at_ms),
        attempt: record.attempt,
        max_attempts: record.max_attempts,
        last_error: record.last_error.clone(),
        principal_id,
        correlation_id,
    }
}

#[cfg(feature = "redis")]
async fn redis_records_for_ids(
    connection: &mut redis::aio::ConnectionManager,
    record_prefix: &str,
    ids: &[String],
) -> Result<Vec<RedisJobRecord>, redis::RedisError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<String> = ids
        .iter()
        .map(|id| redis_record_key(record_prefix, id))
        .collect();
    let bodies: Vec<Option<String>> = redis::cmd("MGET").arg(keys).query_async(connection).await?;
    Ok(bodies
        .into_iter()
        .flatten()
        .filter_map(|body| serde_json::from_str::<RedisJobRecord>(&body).ok())
        .collect())
}

#[cfg(feature = "redis")]
async fn redis_admin_active_list_page(
    connection: &mut redis::aio::ConnectionManager,
    queue_key: &str,
    record_prefix: &str,
    status: JobAdminStatus,
    page: u64,
    per_page: u64,
) -> AutumnResult<JobAdminPage> {
    let page = page.max(1);
    let start = page.saturating_sub(1).saturating_mul(per_page);
    let stop = start.saturating_add(per_page).saturating_sub(1);
    let (ids, total): (Vec<String>, u64) = redis::pipe()
        .cmd("LRANGE")
        .arg(queue_key)
        .arg(start)
        .arg(stop)
        .cmd("LLEN")
        .arg(queue_key)
        .query_async(connection)
        .await
        .map_err(|error| redis_admin_error("read enqueued page", &error))?;
    let records = redis_records_for_ids(connection, record_prefix, &ids)
        .await
        .map_err(|error| redis_admin_error("read enqueued records", &error))?
        .into_iter()
        .map(|record| redis_record_to_admin_record(&record, status))
        .collect();
    Ok(JobAdminPage::new(records, total, page, per_page))
}

#[cfg(feature = "redis")]
async fn redis_admin_running_page(
    connection: &mut redis::aio::ConnectionManager,
    processing_key: &str,
    record_prefix: &str,
    page: u64,
    per_page: u64,
) -> AutumnResult<JobAdminPage> {
    let page = page.max(1);
    let start = page.saturating_sub(1).saturating_mul(per_page);
    let stop = start.saturating_add(per_page).saturating_sub(1);
    let (ids, total): (Vec<String>, u64) = redis::pipe()
        .cmd("ZREVRANGE")
        .arg(processing_key)
        .arg(start)
        .arg(stop)
        .cmd("ZCARD")
        .arg(processing_key)
        .query_async(connection)
        .await
        .map_err(|error| redis_admin_error("read running page", &error))?;
    let mut records: Vec<_> = redis_records_for_ids(connection, record_prefix, &ids)
        .await
        .map_err(|error| redis_admin_error("read running records", &error))?
        .into_iter()
        .map(|record| redis_record_to_admin_record(&record, JobAdminStatus::Running))
        .collect();
    records.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(JobAdminPage::new(records, total, page, per_page))
}

#[cfg(feature = "redis")]
async fn redis_admin_encoded_list_page(
    connection: &mut redis::aio::ConnectionManager,
    list_key: &str,
    status: JobAdminStatus,
    since_ms: Option<u64>,
    page: u64,
    per_page: u64,
    history_limit: usize,
) -> AutumnResult<JobAdminPage> {
    let page = page.max(1);
    let stop = isize::try_from(history_limit.saturating_sub(1)).unwrap_or(isize::MAX);
    let bodies: Vec<String> = redis::cmd("LRANGE")
        .arg(list_key)
        .arg(0)
        .arg(stop)
        .query_async(connection)
        .await
        .map_err(|error| redis_admin_error("read completed/failed list", &error))?;
    let mut records: Vec<_> = bodies
        .into_iter()
        .filter_map(|body| serde_json::from_str::<RedisJobRecord>(&body).ok())
        .filter(|record| since_ms.is_none_or(|since| redis_record_sort_time(record) >= since))
        .collect();
    records.sort_by_key(redis_record_sort_time);
    records.reverse();

    let total = records.len() as u64;
    let start =
        usize::try_from(page.saturating_sub(1).saturating_mul(per_page)).unwrap_or(usize::MAX);
    let take = usize::try_from(per_page).unwrap_or(usize::MAX);
    let page_records = records
        .into_iter()
        .skip(start)
        .take(take)
        .map(|record| redis_record_to_admin_record(&record, status))
        .collect();
    Ok(JobAdminPage::new(page_records, total, page, per_page))
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
record['started_at_ms'] = tonumber(ARGV[2])
record['finished_at_ms'] = nil
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

    let record = match serde_json::from_str::<RedisJobRecord>(&body) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(job_id = %id, error = %error, "invalid durable job record");
            let malformed = serde_json::json!({
                "id": id,
                "error": error.to_string(),
                "raw_payload": body,
            });
            push_json_list_item(connection, &worker_config.dead_key, &malformed).await;
            let _ = redis::cmd("ZREM")
                .arg(&worker_config.processing_key)
                .arg(id)
                .query_async::<usize>(connection)
                .await;
            return Ok(None);
        }
    };
    Ok(Some(record))
}

#[cfg(feature = "redis")]
async fn record_enqueues_for_redis_ids(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
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
        if let Ok(mut record) = serde_json::from_str::<RedisJobRecord>(&body) {
            record.enqueued_at_ms = Some(now_unix_ms());
            record.started_at_ms = None;
            record.finished_at_ms = None;
            clear_redis_claim(&mut record);
            if let Ok(encoded) = encode_redis_record(&record) {
                let key = redis_record_key(&worker_config.record_prefix, &record.id);
                let _ = redis::cmd("SET")
                    .arg(key)
                    .arg(encoded)
                    .query_async::<()>(&mut *connection)
                    .await;
            }
            state.job_registry.record_enqueue(&record.name);
            job_admin.record_requeued(&record.id, record.attempt);
        }
    }

    Ok(())
}

#[cfg(feature = "redis")]
async fn promote_due_redis_retries(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
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

    record_enqueues_for_redis_ids(connection, worker_config, state, job_admin, &promoted).await?;
    Ok(())
}

#[cfg(feature = "redis")]
fn expected_claim_args(record: &RedisJobRecord) -> Option<(&str, u64)> {
    Some((record.claimed_by.as_deref()?, record.claimed_at_ms?))
}

#[cfg(feature = "redis")]
const CLAIMED_REDIS_TRANSITION_SCRIPT: &str = r"
local function trim_dead_history(dead_key, dead_record_prefix, limit)
  local trimmed_records = redis.call('LRANGE', dead_key, limit, -1)
  for _, encoded in ipairs(trimmed_records) do
    local trimmed_ok, trimmed = pcall(cjson.decode, encoded)
    if trimmed_ok and trimmed['id'] then
      redis.call('DEL', dead_record_prefix .. trimmed['id'])
    end
  end
  redis.call('LTRIM', dead_key, 0, limit - 1)
end
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
  redis.call('LPUSH', KEYS[5], ARGV[5])
  redis.call('LTRIM', KEYS[5], 0, tonumber(ARGV[7]) - 1)
  redis.call('DEL', key)
elseif ARGV[4] == 'retry' then
  redis.call('SET', key, ARGV[5])
  redis.call('ZADD', KEYS[3], ARGV[6], ARGV[1])
elseif ARGV[4] == 'dead' then
  redis.call('LPUSH', KEYS[4], ARGV[5])
  redis.call('SET', KEYS[6] .. ARGV[1], ARGV[5])
  trim_dead_history(KEYS[4], KEYS[6], tonumber(ARGV[7]))
  redis.call('DEL', key)
else
  return 0
end
return 1
";

#[cfg(feature = "redis")]
async fn apply_claimed_redis_transition(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    expected: &RedisJobRecord,
    mode: &str,
    encoded_record: Option<String>,
    due_at_ms: Option<u64>,
) -> Result<bool, redis::RedisError> {
    let Some((claimed_by, claimed_at_ms)) = expected_claim_args(expected) else {
        return Ok(false);
    };

    let applied: usize = redis::cmd("EVAL")
        .arg(CLAIMED_REDIS_TRANSITION_SCRIPT)
        .arg(6)
        .arg(&worker_config.processing_key)
        .arg(&worker_config.record_prefix)
        .arg(&worker_config.delayed_key)
        .arg(&worker_config.dead_key)
        .arg(&worker_config.completed_key)
        .arg(&worker_config.dead_record_prefix)
        .arg(&expected.id)
        .arg(claimed_by)
        .arg(claimed_at_ms)
        .arg(mode)
        .arg(encoded_record.unwrap_or_default())
        .arg(due_at_ms.unwrap_or_default())
        .arg(DEFAULT_JOB_ADMIN_HISTORY_LIMIT)
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
    let mut completed = record.clone();
    clear_redis_claim(&mut completed);
    completed.finished_at_ms = Some(now_unix_ms());
    completed.last_error = None;
    let Ok(encoded) = encode_redis_record(&completed) else {
        tracing::warn!(job_id = %record.id, "failed to serialize redis completed record");
        return Ok(false);
    };
    apply_claimed_redis_transition(
        connection,
        worker_config,
        record,
        "success",
        Some(encoded),
        None,
    )
    .await
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
const STALE_REDIS_RECOVERY_SCRIPT: &str = r"
local function trim_dead_history(dead_key, dead_record_prefix, limit)
  local trimmed_records = redis.call('LRANGE', dead_key, limit, -1)
  for _, encoded in ipairs(trimmed_records) do
    local trimmed_ok, trimmed = pcall(cjson.decode, encoded)
    if trimmed_ok and trimmed['id'] then
      redis.call('DEL', dead_record_prefix .. trimmed['id'])
    end
  end
  redis.call('LTRIM', dead_key, 0, limit - 1)
end
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
  redis.call('SET', KEYS[5] .. ARGV[1], ARGV[5])
  trim_dead_history(KEYS[4], KEYS[5], tonumber(ARGV[6]))
  redis.call('DEL', key)
else
  return 0
end
return 1
";

#[cfg(feature = "redis")]
async fn apply_stale_redis_recovery(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    expected: &RedisJobRecord,
    action: &RedisStaleRecovery,
) -> Result<bool, redis::RedisError> {
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
        .arg(STALE_REDIS_RECOVERY_SCRIPT)
        .arg(5)
        .arg(&worker_config.processing_key)
        .arg(&worker_config.record_prefix)
        .arg(&worker_config.queue_key)
        .arg(&worker_config.dead_key)
        .arg(&worker_config.dead_record_prefix)
        .arg(&expected.id)
        .arg(claimed_by)
        .arg(claimed_at_ms)
        .arg(mode)
        .arg(encoded)
        .arg(DEFAULT_JOB_ADMIN_HISTORY_LIMIT)
        .query_async(connection)
        .await?;

    Ok(applied == 1)
}

#[cfg(feature = "redis")]
async fn recover_stale_redis_jobs(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
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
                        job_admin.record_retrying(&requeued.id, error);
                    }
                    state.job_registry.record_enqueue(&requeued.name);
                    job_admin.record_requeued(&requeued.id, requeued.attempt);
                }
                RedisStaleRecovery::DeadLetter(dead) => {
                    let error = dead
                        .last_error
                        .clone()
                        .unwrap_or_else(|| "visibility timeout expired".to_string());
                    state
                        .job_registry
                        .record_failure(&dead.name, error.clone(), true);
                    job_admin.record_failure(&dead.id, error);
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
    job_admin: JobAdminMemoryBackend,
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

            #[allow(clippy::collapsible_if)]
            if retry_promotion_throttle.take_due(std::time::Instant::now()) {
                if let Err(error) =
                    promote_due_redis_retries(&mut connection, &worker_config, &state, &job_admin)
                        .await
                {
                    tracing::warn!(error = %error, "redis job worker retry promotion failed");
                }
            }

            #[allow(clippy::collapsible_if)]
            if stale_recovery_throttle.take_due(std::time::Instant::now()) {
                if let Err(error) =
                    recover_stale_redis_jobs(&mut connection, &worker_config, &state, &job_admin)
                        .await
                {
                    tracing::warn!(error = %error, "redis job worker stale recovery failed");
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
                &job_admin,
                &worker_config,
            )
            .await;
        }
    });

    Ok(())
}

#[cfg(feature = "redis")]
#[allow(clippy::cognitive_complexity)]
async fn settle_failed_redis_job(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    record: &RedisJobRecord,
    error: String,
    outcome: &str,
    job_admin: &JobAdminMemoryBackend,
) {
    let action = prepare_redis_failure_action(record.clone(), error.clone(), now_unix_ms());
    match action {
        RedisFailureAction::Retry(schedule) => {
            match schedule_redis_retry(connection, worker_config, record, &schedule).await {
                Ok(true) => {
                    state
                        .job_registry
                        .record_retry(&schedule.record.name, &error, record.attempt);
                    job_admin.record_retrying(&schedule.record.id, &error);
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
                Ok(true) => {
                    state
                        .job_registry
                        .record_failure(&dead.name, error.clone(), true);
                    job_admin.record_failure(&dead.id, error);
                }
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
    job_admin: &JobAdminMemoryBackend,
) {
    let dead = prepare_redis_panic_dead_letter(record.clone(), error.clone(), now_unix_ms());
    match dead_letter_redis_job(connection, worker_config, record, &dead).await {
        Ok(true) => {
            state
                .job_registry
                .record_failure(&dead.name, error.clone(), true);
            job_admin.record_failure(&dead.id, error);
        }
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
async fn dead_letter_invalid_redis_job(
    connection: &mut redis::aio::ConnectionManager,
    worker_config: &RedisWorkerConfig,
    state: &AppState,
    record: &RedisJobRecord,
    error: &str,
    job_admin: &JobAdminMemoryBackend,
) {
    state
        .job_registry
        .record_failure(&record.name, error.to_owned(), true);
    job_admin.record_failure(&record.id, error.to_owned());
    let mut dead = record.clone();
    clear_redis_claim(&mut dead);
    dead.last_error = Some(error.to_owned());
    let _ = dead_letter_redis_job(connection, worker_config, record, &dead).await;
}

#[cfg(feature = "redis")]
#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
async fn process_redis_job_record(
    connection: &mut redis::aio::ConnectionManager,
    mut record: RedisJobRecord,
    jobs_by_name: &Arc<RwLock<HashMap<String, JobInfo>>>,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
    worker_config: &RedisWorkerConfig,
) {
    if job_admin.try_record_start(&record.id, record.attempt) == JobAdminStartDecision::Canceled {
        state.job_registry.record_cancel(&record.name);
        job_admin.record_cancelled(&record.id);
        let _ = ack_redis_success(connection, worker_config, &record).await;
        return;
    }
    state.job_registry.record_start(&record.name);

    let maybe_info = {
        let guard = jobs_by_name.read().expect("job registry lock poisoned");
        guard
            .get(&record.name)
            .map(|info| (info.handler, info.max_attempts, info.initial_backoff_ms))
    };
    let Some((handler, info_max_attempts, info_backoff_ms)) = maybe_info else {
        dead_letter_invalid_redis_job(
            connection,
            worker_config,
            state,
            &record,
            "unknown job type",
            job_admin,
        )
        .await;
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
        dead_letter_invalid_redis_job(
            connection,
            worker_config,
            state,
            &record,
            "invalid job payload: attempt must be >= 1",
            job_admin,
        )
        .await;
        return;
    }

    let job_span = build_job_consumer_span(&record.name, record.attempt);
    #[cfg(feature = "telemetry-otlp")]
    if let Some(cx) =
        restore_job_trace_context(record.traceparent.as_deref(), record.tracestate.as_deref())
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        let _ = job_span.set_parent(cx);
    }
    let f = run_job_handler(&record.name, handler, state.clone(), record.payload.clone());
    match tracing::Instrument::instrument(f, job_span).await {
        JobExecutionOutcome::Succeeded => {
            match ack_redis_success(connection, worker_config, &record).await {
                Ok(true) => {
                    state.job_registry.record_success(&record.name);
                    job_admin.record_success(&record.id);
                }
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
            settle_failed_redis_job(
                connection,
                worker_config,
                state,
                &record,
                error,
                "failed",
                job_admin,
            )
            .await;
        }
        JobExecutionOutcome::Panicked(error) => {
            tracing::error!(job = %record.name, error = %error, "redis job handler panicked");
            dead_letter_panicked_redis_job(
                connection,
                worker_config,
                state,
                &record,
                error,
                job_admin,
            )
            .await;
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
    let job_admin = JobAdminMemoryBackend::new();
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
    let admin_connection =
        new_redis_connection_manager(&client, "jobs redis admin connection manager")?;

    let queue_key = format!("{}:queue", config.redis.key_prefix);
    let processing_key = format!("{}:processing", config.redis.key_prefix);
    let delayed_key = format!("{}:delayed", config.redis.key_prefix);
    let dead_key = format!("{}:dead", config.redis.key_prefix);
    let completed_key = format!("{}:completed", config.redis.key_prefix);
    let record_prefix = format!("{}:record:", config.redis.key_prefix);
    let dead_record_prefix = format!("{}:dead-record:", config.redis.key_prefix);

    if job_admin_backend(state).is_none() {
        state.insert_extension(JobAdminBackendEntry(Arc::new(RedisJobAdminBackend::new(
            admin_connection,
            queue_key.clone(),
            processing_key.clone(),
            dead_key.clone(),
            completed_key.clone(),
            record_prefix.clone(),
            dead_record_prefix.clone(),
            DEFAULT_JOB_ADMIN_HISTORY_LIMIT,
        ))));
    }

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
        #[cfg(feature = "db")]
        pg_pool: None,
        registry: state.job_registry.clone(),
        job_admin: job_admin.clone(),
        default_max_attempts: config.max_attempts,
        default_initial_backoff_ms: config.initial_backoff_ms,
        per_job_defaults,
        interceptor: state
            .extension::<Arc<dyn crate::interceptor::JobInterceptor>>()
            .map(|arc| (*arc).clone()),
    });

    let worker_count = config.workers.max(1);
    for _ in 0..worker_count {
        spawn_redis_worker(
            &client,
            Arc::clone(&jobs_by_name),
            state.clone(),
            job_admin.clone(),
            shutdown.clone(),
            RedisWorkerConfig {
                queue_key: queue_key.clone(),
                processing_key: processing_key.clone(),
                delayed_key: delayed_key.clone(),
                dead_key: dead_key.clone(),
                completed_key: completed_key.clone(),
                record_prefix: record_prefix.clone(),
                dead_record_prefix: dead_record_prefix.clone(),
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

// ─────────────────────────────────────────────────────────────────────────────
// Postgres job backend (feature = "db")
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "db")]
type PgPool = diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>;

#[cfg(feature = "db")]
const PG_STATUS_ENQUEUED: &str = "enqueued";
#[cfg(feature = "db")]
const PG_STATUS_RUNNING: &str = "running";
#[cfg(feature = "db")]
const PG_STATUS_COMPLETED: &str = "completed";
#[cfg(feature = "db")]
const PG_STATUS_FAILED: &str = "failed";

#[cfg(feature = "db")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PgLifecycleRecord<'a> {
    Success,
    Retry { error: &'a str, attempt: u32 },
    Failure { error: &'a str },
}

#[cfg(feature = "db")]
const fn pg_claim_transition_applied(rows_affected: usize) -> bool {
    rows_affected > 0
}

#[cfg(feature = "db")]
fn record_pg_lifecycle_after_ack(
    ack_applied: bool,
    job_name: &str,
    job_id: &str,
    lifecycle: PgLifecycleRecord<'_>,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
) -> bool {
    if !ack_applied {
        // The claim was evicted by stale-claim recovery before this ack ran.
        // The recovery task already transitioned the row in the database:
        // - non-terminal attempts are requeued (attempt < max_attempts)
        // - terminal attempts are dead-lettered (attempt >= max_attempts)
        // Mirror whichever outcome the worker intended so /actuator metrics stay
        // consistent with the database row.
        if let PgLifecycleRecord::Failure { error } = lifecycle {
            // Stale recovery dead-lettered the row (final attempt).
            state
                .job_registry
                .record_failure(job_name, error.to_owned(), true);
            job_admin.record_failure(job_id, error.to_owned());
        } else {
            // Non-terminal or successful outcome: decrement in_flight and
            // mark as retrying; the row is already back in the queue.
            state
                .job_registry
                .record_retry(job_name, "visibility timeout expired", 0);
            job_admin.record_retrying(job_id, "visibility timeout expired");
        }
        return false;
    }

    match lifecycle {
        PgLifecycleRecord::Success => {
            state.job_registry.record_success(job_name);
            job_admin.record_success(job_id);
        }
        PgLifecycleRecord::Retry { error, attempt } => {
            state.job_registry.record_retry(job_name, error, attempt);
            job_admin.record_retrying(job_id, error);
            // The row is back in autumn_jobs with status='enqueued'; reflect
            // that in the process-local counters so /actuator shows it as queued.
            state.job_registry.record_enqueue(job_name);
            job_admin.record_requeued(job_id, attempt + 1);
        }
        PgLifecycleRecord::Failure { error } => {
            state
                .job_registry
                .record_failure(job_name, error.to_owned(), true);
            job_admin.record_failure(job_id, error.to_owned());
        }
    }

    true
}

#[cfg(feature = "db")]
fn record_pg_lifecycle_ack_result(
    ack_result: AutumnResult<bool>,
    job_name: &str,
    job_id: &str,
    outcome: &str,
    lifecycle: PgLifecycleRecord<'_>,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
) -> bool {
    match ack_result {
        Ok(applied) => {
            let recorded = record_pg_lifecycle_after_ack(
                applied, job_name, job_id, lifecycle, state, job_admin,
            );
            if !recorded {
                tracing::warn!(
                    job = %job_name,
                    job_id = %job_id,
                    outcome = %outcome,
                    "postgres job ack skipped because claim changed"
                );
            }
            recorded
        }
        Err(error) => {
            tracing::warn!(
                job = %job_name,
                job_id = %job_id,
                outcome = %outcome,
                error = %error,
                "postgres job ack failed"
            );
            false
        }
    }
}

#[cfg(feature = "db")]
fn record_pg_row_lifecycle_ack_result(
    ack_result: AutumnResult<bool>,
    row: &PgJobRow,
    outcome: &str,
    lifecycle: PgLifecycleRecord<'_>,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
) -> bool {
    record_pg_lifecycle_ack_result(
        ack_result, &row.name, &row.id, outcome, lifecycle, state, job_admin,
    )
}

#[cfg(feature = "db")]
fn record_pg_cancel_after_ack(
    ack_result: AutumnResult<bool>,
    job_name: &str,
    job_id: &str,
    state: &AppState,
) -> bool {
    match ack_result {
        Ok(true) => {
            state.job_registry.record_cancel(job_name);
            true
        }
        Ok(false) => {
            tracing::warn!(
                job = %job_name,
                job_id = %job_id,
                "postgres job cancel ack skipped because claim changed"
            );
            false
        }
        Err(error) => {
            tracing::warn!(
                job = %job_name,
                job_id = %job_id,
                error = %error,
                "postgres job cancel ack failed"
            );
            false
        }
    }
}

#[cfg(feature = "db")]
fn record_pg_row_cancel_after_ack(
    ack_result: AutumnResult<bool>,
    row: &PgJobRow,
    state: &AppState,
) -> bool {
    record_pg_cancel_after_ack(ack_result, &row.name, &row.id, state)
}

#[cfg(feature = "db")]
const PG_WORKER_IDLE_SLEEP: std::time::Duration = std::time::Duration::from_millis(200);
#[cfg(feature = "db")]
const PG_MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Columns returned by every SELECT from `autumn_jobs` when OTLP is disabled.
#[cfg(all(feature = "db", not(feature = "telemetry-otlp")))]
const PG_JOB_SELECT_COLS: &str = "id, name, payload::TEXT AS payload, status, attempt, \
    max_attempts, initial_backoff_ms, enqueued_at, started_at, finished_at, \
    claimed_by, claimed_at, last_error";

/// Columns returned by every SELECT from `autumn_jobs` when OTLP is enabled.
/// Includes the nullable `traceparent` and `tracestate` columns added by the
/// `add_trace_context_to_jobs` migration.
#[cfg(all(feature = "db", feature = "telemetry-otlp"))]
const PG_JOB_SELECT_COLS: &str = "id, name, payload::TEXT AS payload, status, attempt, \
    max_attempts, initial_backoff_ms, enqueued_at, started_at, finished_at, \
    claimed_by, claimed_at, last_error, traceparent, tracestate";

/// A job row read from the `autumn_jobs` Postgres table.
#[cfg(feature = "db")]
#[derive(diesel::QueryableByName, Debug, Clone)]
#[allow(dead_code)]
struct PgJobRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    payload: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    status: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    attempt: i32,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    max_attempts: i32,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    initial_backoff_ms: i64,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
    enqueued_at: Option<chrono::DateTime<chrono::Utc>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    claimed_by: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
    claimed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    last_error: Option<String>,
    /// W3C `traceparent` captured at enqueue time.
    #[cfg(feature = "telemetry-otlp")]
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    traceparent: Option<String>,
    /// W3C `tracestate` captured at enqueue time.
    #[cfg(feature = "telemetry-otlp")]
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    tracestate: Option<String>,
}

#[cfg(feature = "db")]
impl PgJobRow {
    fn to_admin_record(&self, status: JobAdminStatus) -> JobAdminRecord {
        let payload = serde_json::from_str::<Value>(&self.payload).unwrap_or(Value::Null);
        let (principal_id, correlation_id) = job_payload_identity(&payload);
        JobAdminRecord {
            id: self.id.clone(),
            name: self.name.clone(),
            status,
            enqueued_at: self.enqueued_at.map(format_job_admin_time),
            started_at: self.started_at.map(format_job_admin_time),
            finished_at: self.finished_at.map(format_job_admin_time),
            attempt: u32::try_from(self.attempt).unwrap_or(0),
            max_attempts: u32::try_from(self.max_attempts).unwrap_or(1),
            last_error: self.last_error.clone(),
            principal_id,
            correlation_id,
        }
    }
}

/// A simple count row for admin queries.
#[cfg(feature = "db")]
#[derive(diesel::QueryableByName)]
struct PgCount {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

/// Exponential backoff delay in ms for attempt `attempt` (1-indexed).
#[cfg(feature = "db")]
fn pg_retry_delay_ms(initial_backoff_ms: i64, attempt: i32) -> i64 {
    let exp = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0);
    initial_backoff_ms.saturating_mul(2_i64.saturating_pow(exp))
}

/// Insert a new job row into `autumn_jobs`.
#[cfg(feature = "db")]
async fn pg_enqueue_job(
    pool: &PgPool,
    id: String,
    name: &str,
    payload: Value,
    max_attempts: u32,
    initial_backoff_ms: u64,
) -> AutumnResult<()> {
    use diesel_async::RunQueryDsl as _;

    #[cfg(feature = "telemetry-otlp")]
    let (traceparent, tracestate) = capture_job_trace_context();
    let mut conn = pool
        .get()
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg job pool error: {e}")))?;
    let payload_str = serde_json::to_string(&payload).map_err(|e| {
        AutumnError::internal_server_error_msg(format!("serialize job payload: {e}"))
    })?;
    #[cfg(not(feature = "telemetry-otlp"))]
    let query = diesel::sql_query(
        "INSERT INTO autumn_jobs \
         (id, name, payload, status, attempt, max_attempts, initial_backoff_ms, enqueued_at, run_at) \
         VALUES ($1, $2, $3::JSONB, 'enqueued', 1, $4, $5, NOW(), NOW())",
    )
    .bind::<diesel::sql_types::Text, _>(id)
    .bind::<diesel::sql_types::Text, _>(name)
    .bind::<diesel::sql_types::Text, _>(payload_str)
    .bind::<diesel::sql_types::Integer, _>(i32::try_from(max_attempts).unwrap_or(i32::MAX))
    .bind::<diesel::sql_types::BigInt, _>(i64::try_from(initial_backoff_ms).unwrap_or(i64::MAX));
    #[cfg(feature = "telemetry-otlp")]
    let query = diesel::sql_query(
        "INSERT INTO autumn_jobs \
         (id, name, payload, status, attempt, max_attempts, initial_backoff_ms, \
          enqueued_at, run_at, traceparent, tracestate) \
         VALUES ($1, $2, $3::JSONB, 'enqueued', 1, $4, $5, NOW(), NOW(), $6, $7)",
    )
    .bind::<diesel::sql_types::Text, _>(id)
    .bind::<diesel::sql_types::Text, _>(name)
    .bind::<diesel::sql_types::Text, _>(payload_str)
    .bind::<diesel::sql_types::Integer, _>(i32::try_from(max_attempts).unwrap_or(i32::MAX))
    .bind::<diesel::sql_types::BigInt, _>(i64::try_from(initial_backoff_ms).unwrap_or(i64::MAX))
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(traceparent)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(tracestate);
    query
        .execute(&mut *conn)
        .await
        .map(|_| ())
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg job enqueue failed: {e}")))
}

/// Insert a job into `autumn_jobs` using an **already-open connection**.
///
/// Unlike [`pg_enqueue_job`], this function does not acquire a new connection
/// from the pool. The INSERT participates in whatever transaction the caller
/// has open, so if the caller rolls back, the job row disappears atomically.
#[cfg(feature = "db")]
async fn pg_enqueue_on_conn(
    conn: &mut diesel_async::AsyncPgConnection,
    id: String,
    name: &str,
    payload: Value,
    max_attempts: u32,
    initial_backoff_ms: u64,
) -> AutumnResult<()> {
    use diesel_async::RunQueryDsl as _;

    #[cfg(feature = "telemetry-otlp")]
    let (traceparent, tracestate) = capture_job_trace_context();
    let payload_str = serde_json::to_string(&payload).map_err(|e| {
        AutumnError::internal_server_error_msg(format!("serialize job payload: {e}"))
    })?;
    #[cfg(not(feature = "telemetry-otlp"))]
    let query = diesel::sql_query(
        "INSERT INTO autumn_jobs \
         (id, name, payload, status, attempt, max_attempts, initial_backoff_ms, enqueued_at, run_at) \
         VALUES ($1, $2, $3::JSONB, 'enqueued', 1, $4, $5, NOW(), NOW())",
    )
    .bind::<diesel::sql_types::Text, _>(id)
    .bind::<diesel::sql_types::Text, _>(name)
    .bind::<diesel::sql_types::Text, _>(payload_str)
    .bind::<diesel::sql_types::Integer, _>(i32::try_from(max_attempts).unwrap_or(i32::MAX))
    .bind::<diesel::sql_types::BigInt, _>(i64::try_from(initial_backoff_ms).unwrap_or(i64::MAX));
    #[cfg(feature = "telemetry-otlp")]
    let query = diesel::sql_query(
        "INSERT INTO autumn_jobs \
         (id, name, payload, status, attempt, max_attempts, initial_backoff_ms, \
          enqueued_at, run_at, traceparent, tracestate) \
         VALUES ($1, $2, $3::JSONB, 'enqueued', 1, $4, $5, NOW(), NOW(), $6, $7)",
    )
    .bind::<diesel::sql_types::Text, _>(id)
    .bind::<diesel::sql_types::Text, _>(name)
    .bind::<diesel::sql_types::Text, _>(payload_str)
    .bind::<diesel::sql_types::Integer, _>(i32::try_from(max_attempts).unwrap_or(i32::MAX))
    .bind::<diesel::sql_types::BigInt, _>(i64::try_from(initial_backoff_ms).unwrap_or(i64::MAX))
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(traceparent)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(tracestate);
    query
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg job enqueue failed: {e}")))
}

/// Atomically claim the next ready job with `SELECT … FOR UPDATE SKIP LOCKED`.
///
/// Returns `None` if the queue is empty or all ready rows are locked by
/// competing workers.
#[cfg(feature = "db")]
async fn pg_claim_next_job(pool: &PgPool, worker_id: &str) -> Option<PgJobRow> {
    use diesel::OptionalExtension as _;
    use diesel_async::RunQueryDsl as _;

    let mut conn = pool.get().await.ok()?;
    let sql = format!(
        "UPDATE autumn_jobs \
         SET status = 'running', started_at = NOW(), claimed_by = $1, claimed_at = NOW() \
         WHERE id = ( \
           SELECT id FROM autumn_jobs \
           WHERE status = 'enqueued' AND run_at <= NOW() \
           ORDER BY run_at ASC \
           LIMIT 1 \
           FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING {PG_JOB_SELECT_COLS}"
    );
    diesel::sql_query(sql)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .get_result::<PgJobRow>(&mut *conn)
        .await
        .optional()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "postgres job claim query failed");
            None
        })
}

/// Mark a running job as completed.
#[cfg(feature = "db")]
async fn pg_ack_success(pool: &PgPool, job_id: &str, worker_id: &str) -> AutumnResult<bool> {
    use diesel_async::RunQueryDsl as _;

    let mut conn = pool
        .get()
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg pool error: {e}")))?;
    diesel::sql_query(
        "UPDATE autumn_jobs \
         SET status = 'completed', finished_at = NOW(), \
             claimed_by = NULL, claimed_at = NULL, last_error = NULL \
         WHERE id = $1 AND claimed_by = $2 AND status = 'running'",
    )
    .bind::<diesel::sql_types::Text, _>(job_id)
    .bind::<diesel::sql_types::Text, _>(worker_id)
    .execute(&mut *conn)
    .await
    .map(pg_claim_transition_applied)
    .map_err(|e| AutumnError::internal_server_error_msg(format!("pg job ack failed: {e}")))
}

/// Handle a job failure: schedule a retry with exponential backoff or dead-letter.
#[cfg(feature = "db")]
async fn pg_nack_failure(
    pool: &PgPool,
    job_id: &str,
    worker_id: &str,
    error: &str,
    row: &PgJobRow,
) -> AutumnResult<bool> {
    use diesel_async::RunQueryDsl as _;

    let mut conn = pool
        .get()
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg pool error: {e}")))?;

    if row.attempt < row.max_attempts {
        let delay_ms = pg_retry_delay_ms(row.initial_backoff_ms, row.attempt);
        diesel::sql_query(
            "UPDATE autumn_jobs \
             SET status = 'enqueued', \
                 attempt = attempt + 1, \
                 run_at = NOW() + ($1::BIGINT * INTERVAL '1 millisecond'), \
                 started_at = NULL, \
                 finished_at = NULL, \
                 claimed_by = NULL, \
                 claimed_at = NULL, \
                 last_error = $2 \
             WHERE id = $3 AND claimed_by = $4 AND status = 'running'",
        )
        .bind::<diesel::sql_types::BigInt, _>(delay_ms)
        .bind::<diesel::sql_types::Text, _>(error)
        .bind::<diesel::sql_types::Text, _>(job_id)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .execute(&mut *conn)
        .await
        .map(pg_claim_transition_applied)
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg job retry failed: {e}")))
    } else {
        diesel::sql_query(
            "UPDATE autumn_jobs \
             SET status = 'failed', \
                 finished_at = NOW(), \
                 claimed_by = NULL, \
                 claimed_at = NULL, \
                 last_error = $1 \
             WHERE id = $2 AND claimed_by = $3 AND status = 'running'",
        )
        .bind::<diesel::sql_types::Text, _>(error)
        .bind::<diesel::sql_types::Text, _>(job_id)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .execute(&mut *conn)
        .await
        .map(pg_claim_transition_applied)
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg job dead-letter failed: {e}"))
        })
    }
}

/// Dead-letter a job unconditionally, regardless of remaining attempts.
///
/// Used for panics, which are always terminal regardless of `max_attempts`.
#[cfg(feature = "db")]
async fn pg_ack_dead_letter(
    pool: &PgPool,
    job_id: &str,
    worker_id: &str,
    error: &str,
) -> AutumnResult<bool> {
    use diesel_async::RunQueryDsl as _;

    let mut conn = pool
        .get()
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg pool error: {e}")))?;
    diesel::sql_query(
        "UPDATE autumn_jobs \
         SET status = 'failed', \
             finished_at = NOW(), \
             claimed_by = NULL, \
             claimed_at = NULL, \
             last_error = $1 \
         WHERE id = $2 AND claimed_by = $3 AND status = 'running'",
    )
    .bind::<diesel::sql_types::Text, _>(error)
    .bind::<diesel::sql_types::Text, _>(job_id)
    .bind::<diesel::sql_types::Text, _>(worker_id)
    .execute(&mut *conn)
    .await
    .map(pg_claim_transition_applied)
    .map_err(|e| AutumnError::internal_server_error_msg(format!("pg job dead-letter failed: {e}")))
}

/// Recover jobs whose visibility timeout has expired.
///
/// Uses a single `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED)` so
/// concurrent maintenance tasks from multiple replicas each recover disjoint
/// sets of stale jobs.
#[cfg(feature = "db")]
async fn pg_recover_stale_claims(pool: &PgPool, visibility_timeout_ms: u64) {
    use diesel_async::RunQueryDsl as _;

    let Ok(mut conn) = pool.get().await else {
        tracing::warn!("postgres stale-claim recovery could not acquire connection");
        return;
    };
    let _ = diesel::sql_query(
        "UPDATE autumn_jobs \
         SET \
           status = CASE \
             WHEN attempt < max_attempts THEN 'enqueued'::TEXT \
             ELSE 'failed'::TEXT \
           END, \
           attempt = CASE \
             WHEN attempt < max_attempts THEN attempt + 1 \
             ELSE attempt \
           END, \
           run_at = CASE \
             WHEN attempt < max_attempts THEN NOW() \
             ELSE run_at \
           END, \
           started_at = NULL, \
           finished_at = CASE \
             WHEN attempt >= max_attempts THEN NOW() \
             ELSE NULL \
           END, \
           claimed_by = NULL, \
           claimed_at = NULL, \
           last_error = 'visibility timeout expired' \
         WHERE id IN ( \
           SELECT id FROM autumn_jobs \
           WHERE status = 'running' \
             AND claimed_at < NOW() - ($1::BIGINT * INTERVAL '1 millisecond') \
           FOR UPDATE SKIP LOCKED \
           LIMIT 100 \
         )",
    )
    .bind::<diesel::sql_types::BigInt, _>(i64::try_from(visibility_timeout_ms).unwrap_or(i64::MAX))
    .execute(&mut *conn)
    .await
    .map_err(|e| tracing::warn!(error = %e, "postgres stale claim recovery failed"));
}

/// Execute one claimed job and ack/nack based on the outcome.
#[cfg(feature = "db")]
async fn pg_execute_job(
    row: PgJobRow,
    jobs_by_name: &Arc<RwLock<HashMap<String, JobInfo>>>,
    pool: &PgPool,
    worker_id: &str,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
) {
    let attempt = u32::try_from(row.attempt).unwrap_or(0);
    let max_attempts = u32::try_from(row.max_attempts).unwrap_or(1);

    if job_admin.try_record_start(&row.id, attempt) == JobAdminStartDecision::Canceled {
        let ack = pg_nack_failure(pool, &row.id, worker_id, "canceled by operator", &row).await;
        record_pg_row_cancel_after_ack(ack, &row, state);
        return;
    }
    state.job_registry.record_start(&row.name);

    let payload = serde_json::from_str::<Value>(&row.payload).unwrap_or(Value::Null);
    let handler_opt = jobs_by_name
        .read()
        .expect("job registry lock poisoned")
        .get(&row.name)
        .map(|info| info.handler);

    let Some(handler) = handler_opt else {
        // Dead-letter immediately: no handler will ever exist on this process,
        // so requeueing (pg_nack_failure) would cause every worker to
        // repeatedly claim and discard the job until attempts are exhausted.
        let error = format!("unknown job '{}'", row.name);
        let ack = pg_ack_dead_letter(pool, &row.id, worker_id, &error).await;
        let lifecycle = PgLifecycleRecord::Failure { error: &error };
        record_pg_row_lifecycle_ack_result(ack, &row, "unknown-type", lifecycle, state, job_admin);
        return;
    };

    let job_span = build_job_consumer_span(&row.name, attempt);
    #[cfg(feature = "telemetry-otlp")]
    if let Some(cx) =
        restore_job_trace_context(row.traceparent.as_deref(), row.tracestate.as_deref())
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        let _ = job_span.set_parent(cx);
    }
    let f = run_job_handler(&row.name, handler, state.clone(), payload);
    match tracing::Instrument::instrument(f, job_span).await {
        JobExecutionOutcome::Succeeded => {
            let ack = pg_ack_success(pool, &row.id, worker_id).await;
            record_pg_row_lifecycle_ack_result(
                ack,
                &row,
                "success",
                PgLifecycleRecord::Success,
                state,
                job_admin,
            );
        }
        JobExecutionOutcome::Failed(error) => {
            let lifecycle = if attempt < max_attempts {
                PgLifecycleRecord::Retry {
                    error: &error,
                    attempt,
                }
            } else {
                PgLifecycleRecord::Failure { error: &error }
            };
            let ack = pg_nack_failure(pool, &row.id, worker_id, &error, &row).await;
            record_pg_row_lifecycle_ack_result(ack, &row, "failure", lifecycle, state, job_admin);
        }
        // Panics dead-letter immediately regardless of remaining attempts,
        // matching the local and redis backend behaviour.
        JobExecutionOutcome::Panicked(error) => {
            tracing::error!(job = %row.name, error = %error, "postgres job handler panicked");
            let ack = pg_ack_dead_letter(pool, &row.id, worker_id, &error).await;
            let lifecycle = PgLifecycleRecord::Failure { error: &error };
            record_pg_row_lifecycle_ack_result(ack, &row, "panic", lifecycle, state, job_admin);
        }
    }
}

/// Dedicated maintenance task: runs stale-claim recovery on a fixed interval.
///
/// Spawned once per runtime rather than per-worker so maintenance always runs
/// even when all workers are occupied with long-running jobs.
#[cfg(feature = "db")]
async fn pg_maintenance_loop(
    pool: PgPool,
    visibility_timeout_ms: u64,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut interval = tokio::time::interval(PG_MAINTENANCE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => pg_recover_stale_claims(&pool, visibility_timeout_ms).await,
            () = shutdown.cancelled() => break,
        }
    }
}

#[cfg(feature = "db")]
async fn pg_worker_loop(
    pool: PgPool,
    worker_id: String,
    jobs_by_name: Arc<RwLock<HashMap<String, JobInfo>>>,
    state: AppState,
    job_admin: JobAdminMemoryBackend,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        match pg_claim_next_job(&pool, &worker_id).await {
            Some(row) => {
                pg_execute_job(row, &jobs_by_name, &pool, &worker_id, &state, &job_admin).await;
                if shutdown.is_cancelled() {
                    break;
                }
            }
            None => {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    () = tokio::time::sleep(PG_WORKER_IDLE_SLEEP) => {}
                }
            }
        }
    }
}

/// Postgres-backed job admin dashboard.
#[cfg(feature = "db")]
#[derive(Clone)]
struct PgJobAdminBackend {
    pool: PgPool,
}

#[cfg(feature = "db")]
impl PgJobAdminBackend {
    async fn pg_snapshot(&self, query: &JobAdminQuery) -> AutumnResult<JobAdminSnapshot> {
        let mut conn = self.pool.get().await.map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg admin pool error: {e}"))
        })?;
        let per_page = i64::try_from(query.per_page.clamp(1, 100)).unwrap_or(10);
        let now = chrono::Utc::now();

        let enqueued = pg_admin_page(
            &mut conn,
            PG_STATUS_ENQUEUED,
            "enqueued_at",
            None,
            query.enqueued_page,
            per_page,
        )
        .await?;
        let running = pg_admin_page(
            &mut conn,
            PG_STATUS_RUNNING,
            "started_at",
            None,
            query.running_page,
            per_page,
        )
        .await?;
        let completed = pg_admin_page(
            &mut conn,
            PG_STATUS_COMPLETED,
            "finished_at",
            Some(now - chrono::TimeDelta::hours(24)),
            query.completed_page,
            per_page,
        )
        .await?;
        let failed = pg_admin_page(
            &mut conn,
            PG_STATUS_FAILED,
            "finished_at",
            Some(now - chrono::TimeDelta::days(7)),
            query.failed_page,
            per_page,
        )
        .await?;

        Ok(JobAdminSnapshot {
            enqueued,
            running,
            completed,
            failed,
            schedules: Vec::new(),
            bounded_history_limit: DEFAULT_JOB_ADMIN_HISTORY_LIMIT,
        })
    }

    async fn pg_retry_failed(&self, id: &str) -> AutumnResult<()> {
        use diesel_async::RunQueryDsl as _;

        let mut conn = self.pool.get().await.map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg admin pool error: {e}"))
        })?;
        let updated = diesel::sql_query(
            "UPDATE autumn_jobs \
             SET status = 'enqueued', attempt = 1, run_at = NOW(), enqueued_at = NOW(), \
                 started_at = NULL, finished_at = NULL, \
                 claimed_by = NULL, claimed_at = NULL, last_error = NULL \
             WHERE id = $1 AND status = 'failed'",
        )
        .bind::<diesel::sql_types::Text, _>(id)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg admin retry failed: {e}"))
        })?;
        if updated == 0 {
            return Err(AutumnError::not_found_msg(format!(
                "job '{id}' not found or not in failed state"
            )));
        }
        Ok(())
    }

    async fn pg_discard_failed(&self, id: &str) -> AutumnResult<()> {
        use diesel_async::RunQueryDsl as _;

        let mut conn = self.pool.get().await.map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg admin pool error: {e}"))
        })?;
        let updated = diesel::sql_query(
            "UPDATE autumn_jobs \
             SET status = 'discarded', finished_at = NOW() \
             WHERE id = $1 AND status = 'failed'",
        )
        .bind::<diesel::sql_types::Text, _>(id)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg admin discard failed: {e}"))
        })?;
        if updated == 0 {
            return Err(AutumnError::not_found_msg(format!(
                "job '{id}' not found or not in failed state"
            )));
        }
        Ok(())
    }

    async fn pg_cancel_enqueued(&self, id: &str) -> AutumnResult<()> {
        use diesel_async::RunQueryDsl as _;

        let mut conn = self.pool.get().await.map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg admin pool error: {e}"))
        })?;
        let updated = diesel::sql_query(
            "UPDATE autumn_jobs \
             SET status = 'discarded', finished_at = NOW() \
             WHERE id = $1 AND status = 'enqueued'",
        )
        .bind::<diesel::sql_types::Text, _>(id)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!("pg admin cancel failed: {e}"))
        })?;
        if updated == 0 {
            return Err(AutumnError::not_found_msg(format!(
                "job '{id}' not found or not in enqueued state"
            )));
        }
        Ok(())
    }
}

#[cfg(feature = "db")]
impl JobAdminBackend for PgJobAdminBackend {
    fn snapshot(&self, query: JobAdminQuery) -> JobAdminFuture<'_, JobAdminSnapshot> {
        Box::pin(async move { self.pg_snapshot(&query).await })
    }

    fn retry(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.pg_retry_failed(&id).await })
    }

    fn discard(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.pg_discard_failed(&id).await })
    }

    fn cancel(&self, id: &str) -> JobAdminFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move { self.pg_cancel_enqueued(&id).await })
    }
}

/// Paginated query for one status group in the admin dashboard.
///
/// `sort_col` must be the literal column name that is indexed for this status
/// (e.g. `"enqueued_at"`, `"started_at"`, `"finished_at"`). It is a `&'static
/// str` from our own call sites — never user input — so embedding it via
/// `format!` is safe.
#[cfg(feature = "db")]
async fn pg_admin_page(
    conn: &mut diesel_async::pooled_connection::deadpool::Object<diesel_async::AsyncPgConnection>,
    status: &str,
    sort_col: &'static str,
    since: Option<chrono::DateTime<chrono::Utc>>,
    page: u64,
    per_page: i64,
) -> AutumnResult<JobAdminPage> {
    use diesel_async::RunQueryDsl as _;

    let page = page.max(1);
    let offset = i64::try_from((page - 1).saturating_mul(u64::try_from(per_page).unwrap_or(10)))
        .unwrap_or(0);
    let admin_status = match status {
        PG_STATUS_ENQUEUED => JobAdminStatus::Enqueued,
        PG_STATUS_RUNNING => JobAdminStatus::Running,
        PG_STATUS_COMPLETED => JobAdminStatus::Completed,
        _ => JobAdminStatus::Failed,
    };

    let (total, rows) = if let Some(since) = since {
        let total = diesel::sql_query(format!(
            "SELECT COUNT(*) AS count FROM autumn_jobs \
             WHERE status = $1 AND {sort_col} >= $2"
        ))
        .bind::<diesel::sql_types::Text, _>(status)
        .bind::<diesel::sql_types::Timestamptz, _>(since)
        .get_result::<PgCount>(&mut **conn)
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg admin count: {e}")))?
        .count;

        let rows = diesel::sql_query(format!(
            "SELECT {PG_JOB_SELECT_COLS} FROM autumn_jobs \
             WHERE status = $1 AND {sort_col} >= $2 \
             ORDER BY {sort_col} DESC \
             LIMIT $3 OFFSET $4"
        ))
        .bind::<diesel::sql_types::Text, _>(status)
        .bind::<diesel::sql_types::Timestamptz, _>(since)
        .bind::<diesel::sql_types::BigInt, _>(per_page)
        .bind::<diesel::sql_types::BigInt, _>(offset)
        .load::<PgJobRow>(&mut **conn)
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg admin page: {e}")))?;

        (total, rows)
    } else {
        let total =
            diesel::sql_query("SELECT COUNT(*) AS count FROM autumn_jobs WHERE status = $1")
                .bind::<diesel::sql_types::Text, _>(status)
                .get_result::<PgCount>(&mut **conn)
                .await
                .map_err(|e| {
                    AutumnError::internal_server_error_msg(format!("pg admin count: {e}"))
                })?
                .count;

        let rows = diesel::sql_query(format!(
            "SELECT {PG_JOB_SELECT_COLS} FROM autumn_jobs \
             WHERE status = $1 \
             ORDER BY {sort_col} DESC NULLS LAST \
             LIMIT $2 OFFSET $3"
        ))
        .bind::<diesel::sql_types::Text, _>(status)
        .bind::<diesel::sql_types::BigInt, _>(per_page)
        .bind::<diesel::sql_types::BigInt, _>(offset)
        .load::<PgJobRow>(&mut **conn)
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(format!("pg admin page: {e}")))?;

        (total, rows)
    };

    let records = rows
        .iter()
        .map(|r| r.to_admin_record(admin_status))
        .collect();
    Ok(JobAdminPage::new(
        records,
        u64::try_from(total).unwrap_or(0),
        page,
        u64::try_from(per_page).unwrap_or(10),
    ))
}

/// Start the Postgres job runtime.
#[cfg(feature = "db")]
fn start_postgres_runtime(
    jobs: Vec<JobInfo>,
    state: &AppState,
    shutdown: &tokio_util::sync::CancellationToken,
    config: &crate::config::JobConfig,
) -> AutumnResult<()> {
    let pool = state.pool().cloned().ok_or_else(|| {
        AutumnError::internal_server_error(std::io::Error::other(
            "jobs.backend=postgres requires a configured database; \
             set database.url or call AppBuilder::with_pool()",
        ))
    })?;

    let job_admin = JobAdminMemoryBackend::new();
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

    if job_admin_backend(state).is_none() {
        state.insert_extension(JobAdminBackendEntry(Arc::new(PgJobAdminBackend {
            pool: pool.clone(),
        })));
    }

    init_global_job_client(JobClient {
        local_sender: None,
        #[cfg(feature = "redis")]
        redis: None,
        pg_pool: Some(pool.clone()),
        registry: state.job_registry.clone(),
        job_admin: job_admin.clone(),
        default_max_attempts: config.max_attempts,
        default_initial_backoff_ms: config.initial_backoff_ms,
        per_job_defaults,
        interceptor: state
            .extension::<Arc<dyn crate::interceptor::JobInterceptor>>()
            .map(|arc| (*arc).clone()),
    });

    let visibility_timeout_ms = config.postgres.visibility_timeout_ms;
    let worker_count = config.workers.max(1);

    // Single maintenance task shared across all workers.
    {
        let pool = pool.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            pg_maintenance_loop(pool, visibility_timeout_ms, shutdown).await;
        });
    }

    for _ in 0..worker_count {
        let pool = pool.clone();
        let jobs_by_name = Arc::clone(&jobs_by_name);
        let state = state.clone();
        let job_admin = job_admin.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let worker_id = format!("{}:{}", std::process::id(), uuid::Uuid::new_v4());
            pg_worker_loop(pool, worker_id, jobs_by_name, state, job_admin, shutdown).await;
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
    #[cfg(feature = "redis")]
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
    async fn job_admin_backend_lists_and_operates_failed_jobs() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        let enqueued_id = backend.record_enqueue_for_test(
            "send_email",
            serde_json::json!({
                "user_id": 42,
                "correlation_id": "req-123",
                "subject": "Welcome"
            }),
            1,
            5,
        );
        let running_id = backend.record_enqueue_for_test("reindex", serde_json::json!({}), 1, 3);
        backend.record_start_for_test(&running_id, 1);
        let completed_id = backend.record_enqueue_for_test("digest", serde_json::json!({}), 1, 3);
        backend.record_start_for_test(&completed_id, 1);
        backend.record_success_for_test(&completed_id);
        let failed_id =
            backend.record_enqueue_for_test("send_email", serde_json::json!({"user_id": 7}), 2, 5);
        backend.record_start_for_test(&failed_id, 2);
        backend.record_failure_for_test(&failed_id, "smtp refused recipient");

        let snapshot = backend
            .snapshot(JobAdminQuery {
                enqueued_page: 1,
                running_page: 1,
                completed_page: 1,
                failed_page: 1,
                per_page: 10,
            })
            .await
            .expect("snapshot should render");

        assert_eq!(snapshot.enqueued.records[0].id, enqueued_id);
        assert_eq!(
            snapshot.enqueued.records[0].principal_id.as_deref(),
            Some("42")
        );
        assert_eq!(
            snapshot.enqueued.records[0].correlation_id.as_deref(),
            Some("req-123")
        );
        assert_eq!(snapshot.running.records[0].id, running_id);
        assert_eq!(snapshot.completed.records[0].id, completed_id);
        assert_eq!(snapshot.failed.records[0].id, failed_id);
        assert_eq!(
            snapshot.failed.records[0].last_error.as_deref(),
            Some("smtp refused recipient")
        );

        backend
            .discard(&failed_id)
            .await
            .expect("failed job should be discardable");
        backend
            .cancel(&enqueued_id)
            .await
            .expect("enqueued job should be cancelable");
        assert_eq!(
            backend.try_record_start(&enqueued_id, 1),
            JobAdminStartDecision::Canceled,
            "canceled jobs must not race into running"
        );

        let snapshot = backend
            .snapshot(JobAdminQuery::default())
            .await
            .expect("snapshot after operations");
        assert!(snapshot.failed.records.is_empty());
        assert!(snapshot.enqueued.records.is_empty());
    }

    #[tokio::test]
    async fn job_admin_retry_reenqueues_failed_payload() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let backend = JobAdminMemoryBackend::new_for_test(32);
        let (tx, mut rx) = mpsc::channel(1);
        init_global_job_client(JobClient {
            local_sender: Some(tx),
            #[cfg(feature = "redis")]
            redis: None,
            #[cfg(feature = "db")]
            pg_pool: None,
            registry: crate::actuator::JobRegistry::new(),
            job_admin: backend.clone(),
            default_max_attempts: 5,
            default_initial_backoff_ms: 250,
            per_job_defaults: HashMap::from([("send_email".to_string(), (5, 250))]),
            interceptor: None,
        });

        let failed_id = backend.record_enqueue_for_test(
            "send_email",
            serde_json::json!({
                "user_id": 7,
                "correlation_id": "req-retry"
            }),
            2,
            5,
        );
        backend.record_start_for_test(&failed_id, 2);
        backend.record_failure_for_test(&failed_id, "smtp refused recipient");

        backend
            .retry(&failed_id)
            .await
            .expect("failed job should be retried");
        let queued = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("retry should enqueue promptly")
            .expect("retry should enqueue a job");

        assert_eq!(queued.name, "send_email");
        assert_eq!(queued.attempt, 1);
        assert_eq!(queued.max_attempts, 5);
        assert_eq!(queued.payload["user_id"], 7);
        assert_eq!(queued.payload["correlation_id"], "req-retry");

        let snapshot = backend
            .snapshot(JobAdminQuery::default())
            .await
            .expect("snapshot after retry");
        assert!(snapshot.failed.records.is_empty());
        assert_eq!(snapshot.enqueued.total, 1);

        clear_global_job_client();
    }

    #[tokio::test]
    async fn job_admin_retry_restores_failed_record_when_enqueue_fails() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let backend = JobAdminMemoryBackend::new_for_test(32);
        let registry = crate::actuator::JobRegistry::new();
        registry.register("send_email");
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        init_global_job_client(JobClient {
            local_sender: Some(tx),
            #[cfg(feature = "redis")]
            redis: None,
            #[cfg(feature = "db")]
            pg_pool: None,
            registry: registry.clone(),
            job_admin: backend.clone(),
            default_max_attempts: 5,
            default_initial_backoff_ms: 250,
            per_job_defaults: HashMap::from([("send_email".to_string(), (5, 250))]),
            interceptor: None,
        });

        let failed_id =
            backend.record_enqueue_for_test("send_email", serde_json::json!({"user_id": 7}), 2, 5);
        backend.record_start_for_test(&failed_id, 2);
        backend.record_failure_for_test(&failed_id, "smtp refused recipient");

        let error = backend
            .retry(&failed_id)
            .await
            .expect_err("closed worker channel should make retry enqueue fail");
        assert!(
            error.to_string().contains("failed to enqueue job"),
            "unexpected retry error: {error}"
        );

        let snapshot = backend
            .snapshot(JobAdminQuery::default())
            .await
            .expect("snapshot after failed retry enqueue");
        assert_eq!(snapshot.failed.total, 1);
        assert_eq!(snapshot.failed.records[0].id, failed_id);
        assert_eq!(
            snapshot.failed.records[0].last_error.as_deref(),
            Some("smtp refused recipient")
        );
        assert_eq!(snapshot.enqueued.total, 0);
        let status = registry.snapshot()["send_email"].clone();
        assert_eq!(status.queued, 0);

        clear_global_job_client();
    }

    #[tokio::test]
    async fn job_admin_retry_claims_failed_record_before_enqueueing() {
        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let backend = JobAdminMemoryBackend::new_for_test(32);
        let (tx, mut rx) = mpsc::channel(2);
        init_global_job_client(JobClient {
            local_sender: Some(tx),
            #[cfg(feature = "redis")]
            redis: None,
            #[cfg(feature = "db")]
            pg_pool: None,
            registry: crate::actuator::JobRegistry::new(),
            job_admin: backend.clone(),
            default_max_attempts: 5,
            default_initial_backoff_ms: 250,
            per_job_defaults: HashMap::from([("send_email".to_string(), (5, 250))]),
            interceptor: None,
        });

        let failed_id =
            backend.record_enqueue_for_test("send_email", serde_json::json!({"user_id": 7}), 2, 5);
        backend.record_start_for_test(&failed_id, 2);
        backend.record_failure_for_test(&failed_id, "smtp refused recipient");

        let (first, second) = tokio::join!(backend.retry(&failed_id), backend.retry(&failed_id));
        assert!(
            first.is_ok() ^ second.is_ok(),
            "exactly one concurrent retry should claim the failed job"
        );
        let queued = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("one retry should enqueue promptly")
            .expect("one retry should enqueue a job");
        assert_eq!(queued.name, "send_email");
        assert!(timeout(Duration::from_millis(25), rx.recv()).await.is_err());

        clear_global_job_client();
    }

    #[tokio::test]
    async fn job_admin_retry_payload_claim_is_single_use() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        let failed_id =
            backend.record_enqueue_for_test("send_email", serde_json::json!({"user_id": 7}), 2, 5);
        backend.record_start_for_test(&failed_id, 2);
        backend.record_failure_for_test(&failed_id, "smtp refused recipient");

        let first = backend
            .retry_payload(&failed_id)
            .expect("first retry claim should return the payload");
        assert_eq!(first.0, "send_email");
        let second = backend
            .retry_payload(&failed_id)
            .expect_err("second retry claim must be rejected before enqueue");
        assert!(
            second
                .to_string()
                .contains("only failed jobs can be retried"),
            "unexpected second retry error: {second}"
        );
    }

    #[tokio::test]
    async fn run_job_handler_reports_immediate_panics() {
        let state = AppState::for_test().with_profile("dev");
        let outcome = run_job_handler(
            "test_job",
            instantly_panicking_handler,
            state,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            outcome,
            JobExecutionOutcome::Panicked("job handler panicked: panic before future".to_string())
        );
    }

    #[tokio::test]
    async fn run_job_handler_catches_interceptor_setup_panics() {
        struct PanickingJobInterceptor;
        impl crate::interceptor::JobInterceptor for PanickingJobInterceptor {
            fn intercept_enqueue<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                next
            }

            fn intercept_execute<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                _next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                panic!("interceptor execution setup panicked")
            }
        }

        fn success_handler(
            _state: AppState,
            _payload: Value,
        ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
            Box::pin(async move { Ok(()) })
        }

        let state = AppState::for_test().with_profile("dev");
        state.insert_extension(
            Arc::new(PanickingJobInterceptor) as Arc<dyn crate::interceptor::JobInterceptor>
        );

        let outcome =
            run_job_handler("test_job", success_handler, state, serde_json::json!({})).await;

        assert_eq!(
            outcome,
            JobExecutionOutcome::Panicked(
                "job handler panicked: interceptor execution setup panicked".to_string()
            )
        );
    }

    #[tokio::test]
    async fn run_job_handler_interceptor_short_circuit_prevents_sync_execution() {
        struct ShortCircuitInterceptor;
        impl crate::interceptor::JobInterceptor for ShortCircuitInterceptor {
            fn intercept_enqueue<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                next
            }

            fn intercept_execute<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                _next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                Box::pin(async move {
                    Err(crate::AutumnError::bad_request_msg(
                        "blocked by interceptor",
                    ))
                })
            }
        }

        static SYNC_CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

        fn side_effect_handler(
            _state: AppState,
            _payload: Value,
        ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
            SYNC_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move { Ok(()) })
        }

        let state = AppState::for_test().with_profile("dev");
        state.insert_extension(
            Arc::new(ShortCircuitInterceptor) as Arc<dyn crate::interceptor::JobInterceptor>
        );

        SYNC_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);

        let outcome = run_job_handler(
            "test_job",
            side_effect_handler,
            state,
            serde_json::json!({}),
        )
        .await;

        assert_eq!(
            outcome,
            JobExecutionOutcome::Failed("blocked by interceptor".to_string())
        );

        assert_eq!(SYNC_CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn job_client_enqueue_catches_interceptor_setup_panic() {
        struct PanickingEnqueueInterceptor;
        impl crate::interceptor::JobInterceptor for PanickingEnqueueInterceptor {
            fn intercept_enqueue<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                _next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                panic!("interceptor enqueue setup panicked")
            }

            fn intercept_execute<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                next
            }
        }

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let client = JobClient {
            local_sender: Some(tx),
            #[cfg(feature = "redis")]
            redis: None,
            #[cfg(feature = "db")]
            pg_pool: None,
            registry: crate::actuator::JobRegistry::new(),
            job_admin: JobAdminMemoryBackend::new_for_test(32),
            default_max_attempts: 3,
            default_initial_backoff_ms: 100,
            per_job_defaults: std::collections::HashMap::from([(
                "test_job".to_string(),
                (3_u32, 100_u64),
            )]),
            interceptor: Some(Arc::new(PanickingEnqueueInterceptor)),
        };

        let res = client.enqueue("test_job", serde_json::json!({})).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("job enqueue panicked: interceptor enqueue setup panicked"),
            "expected panic error message, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn job_client_enqueue_catches_interceptor_async_panic() {
        struct AsyncPanickingEnqueueInterceptor;
        impl crate::interceptor::JobInterceptor for AsyncPanickingEnqueueInterceptor {
            fn intercept_enqueue<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                _next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                Box::pin(async move { panic!("interceptor enqueue async panicked") })
            }

            fn intercept_execute<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                next
            }
        }

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let client = JobClient {
            local_sender: Some(tx),
            #[cfg(feature = "redis")]
            redis: None,
            #[cfg(feature = "db")]
            pg_pool: None,
            registry: crate::actuator::JobRegistry::new(),
            job_admin: JobAdminMemoryBackend::new_for_test(32),
            default_max_attempts: 3,
            default_initial_backoff_ms: 100,
            per_job_defaults: std::collections::HashMap::from([(
                "test_job".to_string(),
                (3_u32, 100_u64),
            )]),
            interceptor: Some(Arc::new(AsyncPanickingEnqueueInterceptor)),
        };

        let res = client.enqueue("test_job", serde_json::json!({})).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("job enqueue panicked: interceptor enqueue async panicked"),
            "expected panic error message, got: {err_msg}"
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
    async fn test_interceptor_rejection_rolls_back_enqueue_bookkeeping() {
        struct RejectingInterceptor;
        impl crate::interceptor::JobInterceptor for RejectingInterceptor {
            fn intercept_enqueue<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                _next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                Box::pin(async move {
                    Err(crate::AutumnError::bad_request_msg(
                        "blocked by interceptor",
                    ))
                })
            }

            fn intercept_execute<'a>(
                &'a self,
                _name: &'a str,
                _payload: &'a serde_json::Value,
                next: std::pin::Pin<
                    Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
                >,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
            > {
                next
            }
        }

        let _guard = global_job_runtime_test_lock().lock().await;
        clear_global_job_client();

        let state = AppState::for_test().with_profile("dev");
        state.insert_extension(
            Arc::new(RejectingInterceptor) as Arc<dyn crate::interceptor::JobInterceptor>
        );

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

        let res = enqueue("noop", serde_json::json!({})).await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "blocked by interceptor");

        // The bookkeeping must be rolled back!
        let snapshot = state.job_registry().snapshot();
        assert_eq!(snapshot["noop"].queued, 0);

        let admin = job_admin_backend(&state).unwrap();
        let admin_snapshot = admin.snapshot(JobAdminQuery::default()).await.unwrap();
        assert_eq!(admin_snapshot.enqueued.total, 0);

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
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        let job_id = job_admin.record_enqueue_for_test("panic", serde_json::json!({}), 1, 3);
        execute_local_job(
            QueuedJob {
                id: job_id,
                name: "panic".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 3,
                initial_backoff_ms: 1,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            &jobs_by_name,
            &tx,
            &state,
            &job_admin,
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
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        let job_id = job_admin.record_enqueue_for_test("flaky", serde_json::json!({}), 1, 2);
        execute_local_job(
            QueuedJob {
                id: job_id.clone(),
                name: "flaky".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 2,
                initial_backoff_ms: 1,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            &jobs_by_name,
            &tx,
            &state,
            &job_admin,
        )
        .await;

        let retried = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("retry should be scheduled")
            .expect("retry payload should be sent");
        assert_eq!(retried.id, job_id);
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
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        let job_id = job_admin.record_enqueue_for_test("flaky", serde_json::json!({}), 1, 1);
        execute_local_job(
            QueuedJob {
                id: job_id,
                name: "flaky".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 1,
                initial_backoff_ms: 1,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            &jobs_by_name,
            &tx,
            &state,
            &job_admin,
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
    async fn job_admin_local_retriable_failure_is_not_operator_retryable_failed_work() {
        let state = AppState::for_test().with_profile("dev");
        state.job_registry().register("flaky");
        state.job_registry().record_enqueue("flaky");

        let mut jobs = HashMap::new();
        jobs.insert(
            "flaky".to_string(),
            JobInfo {
                name: "flaky".to_string(),
                max_attempts: 2,
                initial_backoff_ms: 60_000,
                handler: always_fail_handler,
            },
        );
        let jobs_by_name = Arc::new(RwLock::new(jobs));

        let (tx, mut rx) = mpsc::channel(1);
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        let job_id = job_admin.record_enqueue_for_test("flaky", serde_json::json!({}), 1, 2);
        execute_local_job(
            QueuedJob {
                id: job_id.clone(),
                name: "flaky".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 2,
                initial_backoff_ms: 60_000,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            &jobs_by_name,
            &tx,
            &state,
            &job_admin,
        )
        .await;

        let snapshot = job_admin
            .snapshot(JobAdminQuery::default())
            .await
            .expect("job admin snapshot");
        assert!(
            snapshot.failed.records.is_empty(),
            "automatic retries must stay out of terminal failed work"
        );
        let retry_error = job_admin
            .retry(&job_id)
            .await
            .expect_err("operator retry must reject sleeping automatic retries");
        assert!(
            retry_error
                .to_string()
                .contains("only failed jobs can be retried"),
            "unexpected retry error: {retry_error}"
        );
        assert!(timeout(Duration::from_millis(25), rx.recv()).await.is_err());
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
            enqueued_at_ms: Some(1_000),
            started_at_ms: None,
            finished_at_ms: None,
            claimed_by: None,
            claimed_at_ms: None,
            last_error: None,
            #[cfg(feature = "telemetry-otlp")]
            traceparent: None,
            #[cfg(feature = "telemetry-otlp")]
            tracestate: None,
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

        let dead =
            prepare_redis_panic_dead_letter(record, "job handler panicked".to_string(), 50_000);

        assert_eq!(dead.attempt, 1);
        assert_eq!(dead.max_attempts, 3);
        assert_eq!(dead.claimed_by, None);
        assert_eq!(dead.claimed_at_ms, None);
        assert_eq!(dead.finished_at_ms, Some(50_000));
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
    #[test]
    fn redis_dead_letter_scripts_delete_trimmed_dead_record_metadata() {
        assert!(
            CLAIMED_REDIS_TRANSITION_SCRIPT
                .contains("trim_dead_history(KEYS[4], KEYS[6], tonumber(ARGV[7]))"),
            "claimed-job dead-letter trim should delete metadata for records beyond the history limit"
        );
        assert!(
            STALE_REDIS_RECOVERY_SCRIPT
                .contains("trim_dead_history(KEYS[4], KEYS[5], tonumber(ARGV[6]))"),
            "stale-recovery dead-letter trim should delete metadata for records beyond the history limit"
        );
        assert!(
            CLAIMED_REDIS_TRANSITION_SCRIPT
                .matches("redis.call('DEL', dead_record_prefix .. trimmed['id'])")
                .count()
                >= 1,
            "claimed-job dead-letter script should remove trimmed per-id metadata"
        );
        assert!(
            STALE_REDIS_RECOVERY_SCRIPT
                .matches("redis.call('DEL', dead_record_prefix .. trimmed['id'])")
                .count()
                >= 1,
            "stale-recovery dead-letter script should remove trimmed per-id metadata"
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
            completed_key: format!("{prefix}:completed"),
            record_prefix: format!("{prefix}:record:"),
            dead_record_prefix: format!("{prefix}:dead-record:"),
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
                uuid::Uuid::new_v4().to_string(),
                "send_email",
                serde_json::json!({ "user_id": 42 }),
                max_attempts,
                1,
            )
            .await
            .unwrap();
    }

    #[cfg(feature = "redis")]
    struct RedisAdminSeedRecords {
        enqueued: RedisJobRecord,
        running: RedisJobRecord,
        completed: RedisJobRecord,
        failed_retry: RedisJobRecord,
        failed_discard: RedisJobRecord,
    }

    #[cfg(feature = "redis")]
    fn redis_admin_test_backend(
        client: &redis::Client,
        worker_config: &RedisWorkerConfig,
    ) -> RedisJobAdminBackend {
        let admin_connection = new_redis_connection_manager(client, "test redis admin").unwrap();
        RedisJobAdminBackend::new(
            admin_connection,
            worker_config.queue_key.clone(),
            worker_config.processing_key.clone(),
            worker_config.dead_key.clone(),
            worker_config.completed_key.clone(),
            worker_config.record_prefix.clone(),
            worker_config.dead_record_prefix.clone(),
            128,
        )
    }

    #[cfg(feature = "redis")]
    fn redis_admin_seed_records(now: u64) -> RedisAdminSeedRecords {
        RedisAdminSeedRecords {
            enqueued: RedisJobRecord {
                id: "job-enqueued".to_string(),
                name: "send_email".to_string(),
                payload: serde_json::json!({"user_id": 42, "correlation_id": "req-redis"}),
                attempt: 1,
                max_attempts: 5,
                initial_backoff_ms: 250,
                enqueued_at_ms: Some(now.saturating_sub(4_000)),
                started_at_ms: None,
                finished_at_ms: None,
                claimed_by: None,
                claimed_at_ms: None,
                last_error: None,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            running: RedisJobRecord {
                id: "job-running".to_string(),
                name: "reindex".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 3,
                initial_backoff_ms: 250,
                enqueued_at_ms: Some(now.saturating_sub(3_000)),
                started_at_ms: Some(now.saturating_sub(2_000)),
                finished_at_ms: None,
                claimed_by: Some("worker-a".to_string()),
                claimed_at_ms: Some(now.saturating_sub(2_000)),
                last_error: None,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            completed: RedisJobRecord {
                id: "job-completed".to_string(),
                name: "digest".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 3,
                initial_backoff_ms: 250,
                enqueued_at_ms: Some(now.saturating_sub(3_000)),
                started_at_ms: Some(now.saturating_sub(2_000)),
                finished_at_ms: Some(now.saturating_sub(1_000)),
                claimed_by: None,
                claimed_at_ms: None,
                last_error: None,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            failed_retry: RedisJobRecord {
                id: "job-failed-retry".to_string(),
                name: "send_email".to_string(),
                payload: serde_json::json!({ "user_id": 7 }),
                attempt: 5,
                max_attempts: 5,
                initial_backoff_ms: 250,
                enqueued_at_ms: Some(now.saturating_sub(4_000)),
                started_at_ms: Some(now.saturating_sub(3_000)),
                finished_at_ms: Some(now.saturating_sub(500)),
                claimed_by: None,
                claimed_at_ms: None,
                last_error: Some("smtp refused recipient".to_string()),
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            failed_discard: RedisJobRecord {
                id: "job-failed-discard".to_string(),
                name: "webhook".to_string(),
                payload: serde_json::json!({}),
                attempt: 2,
                max_attempts: 2,
                initial_backoff_ms: 250,
                enqueued_at_ms: Some(now.saturating_sub(4_000)),
                started_at_ms: Some(now.saturating_sub(3_000)),
                finished_at_ms: Some(now.saturating_sub(250)),
                claimed_by: None,
                claimed_at_ms: None,
                last_error: Some("endpoint returned 410".to_string()),
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
        }
    }

    #[cfg(feature = "redis")]
    async fn redis_store_active_admin_record(
        connection: &mut redis::aio::ConnectionManager,
        worker_config: &RedisWorkerConfig,
        record: &RedisJobRecord,
        now: u64,
    ) {
        use redis::AsyncCommands as _;

        connection
            .set::<_, _, ()>(
                redis_record_key(&worker_config.record_prefix, &record.id),
                encode_redis_record(record).unwrap(),
            )
            .await
            .unwrap();
        match record.started_at_ms {
            Some(_) => {
                connection
                    .zadd::<_, _, _, ()>(
                        &worker_config.processing_key,
                        &record.id,
                        now.saturating_add(30_000),
                    )
                    .await
                    .unwrap();
            }
            None => {
                connection
                    .lpush::<_, _, ()>(&worker_config.queue_key, &record.id)
                    .await
                    .unwrap();
            }
        }
    }

    #[cfg(feature = "redis")]
    async fn redis_store_history_admin_record(
        connection: &mut redis::aio::ConnectionManager,
        worker_config: &RedisWorkerConfig,
        record: &RedisJobRecord,
        failed: bool,
    ) {
        use redis::AsyncCommands as _;

        let encoded = encode_redis_record(record).unwrap();
        if failed {
            connection
                .lpush::<_, _, ()>(&worker_config.dead_key, &encoded)
                .await
                .unwrap();
            connection
                .set::<_, _, ()>(
                    format!("{}{}", worker_config.dead_record_prefix, record.id),
                    encoded,
                )
                .await
                .unwrap();
        } else {
            connection
                .lpush::<_, _, ()>(&worker_config.completed_key, encoded)
                .await
                .unwrap();
        }
    }

    #[cfg(feature = "redis")]
    async fn seed_redis_admin_storage(
        connection: &mut redis::aio::ConnectionManager,
        worker_config: &RedisWorkerConfig,
        now: u64,
    ) -> RedisAdminSeedRecords {
        let records = redis_admin_seed_records(now);
        redis_store_active_admin_record(connection, worker_config, &records.enqueued, now).await;
        redis_store_active_admin_record(connection, worker_config, &records.running, now).await;
        redis_store_history_admin_record(connection, worker_config, &records.completed, false)
            .await;
        redis_store_history_admin_record(connection, worker_config, &records.failed_retry, true)
            .await;
        redis_store_history_admin_record(connection, worker_config, &records.failed_discard, true)
            .await;
        records
    }

    #[cfg(feature = "redis")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires Docker (testcontainers)"]
    async fn redis_job_admin_dashboard_reads_cluster_storage_and_operates() {
        use redis::AsyncCommands as _;

        let (_container, client) = redis_test_client().await;
        let worker_config = redis_test_worker_config("autumn:test:admin", "worker-a", 30_000);
        let backend = redis_admin_test_backend(&client, &worker_config);
        let mut connection = new_redis_connection_manager(&client, "test redis setup").unwrap();
        let records =
            seed_redis_admin_storage(&mut connection, &worker_config, now_unix_ms()).await;

        let snapshot = backend
            .snapshot(JobAdminQuery {
                enqueued_page: 1,
                running_page: 1,
                completed_page: 1,
                failed_page: 1,
                per_page: 10,
            })
            .await
            .expect("redis dashboard snapshot");
        assert_eq!(snapshot.enqueued.records[0].id, records.enqueued.id);
        assert_eq!(
            snapshot.enqueued.records[0].correlation_id.as_deref(),
            Some("req-redis")
        );
        assert_eq!(snapshot.running.records[0].id, records.running.id);
        assert_eq!(snapshot.completed.records[0].id, records.completed.id);
        assert_eq!(snapshot.failed.total, 2);

        backend
            .cancel(&records.enqueued.id)
            .await
            .expect("enqueued redis job should be cancelable");
        backend
            .retry(&records.failed_retry.id)
            .await
            .expect("failed redis job should be retryable");
        backend
            .discard(&records.failed_discard.id)
            .await
            .expect("failed redis job should be discardable");

        let queue_len: usize = connection.llen(&worker_config.queue_key).await.unwrap();
        let dead_len: usize = connection.llen(&worker_config.dead_key).await.unwrap();
        assert_eq!(queue_len, 1, "retry should enqueue a replacement job");
        assert_eq!(dead_len, 0, "retry and discard should clear failed jobs");
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
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        state.job_registry().register("send_email");
        state.job_registry().record_enqueue("send_email");
        process_redis_job_record(
            &mut connection,
            record,
            &redis_jobs_by_name(redis_counting_success_handler, 2),
            &state,
            &job_admin,
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
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        state.job_registry().register("send_email");
        state.job_registry().record_enqueue("send_email");
        let jobs = redis_jobs_by_name(redis_counting_failure_handler, 2);

        let first = claim_next_redis_job(&mut connection, &worker_config)
            .await
            .unwrap()
            .expect("first attempt should be claimed");
        process_redis_job_record(
            &mut connection,
            first,
            &jobs,
            &state,
            &job_admin,
            &worker_config,
        )
        .await;
        let delayed_count: usize = connection.zcard(&worker_config.delayed_key).await.unwrap();
        let processing_count: usize = connection
            .zcard(&worker_config.processing_key)
            .await
            .unwrap();
        assert_eq!(delayed_count, 1);
        assert_eq!(processing_count, 0);

        tokio::time::sleep(Duration::from_millis(5)).await;
        promote_due_redis_retries(&mut connection, &worker_config, &state, &job_admin)
            .await
            .unwrap();
        let queued_count: usize = connection.llen(&worker_config.queue_key).await.unwrap();
        assert_eq!(queued_count, 1);

        let second = claim_next_redis_job(&mut connection, &worker_config)
            .await
            .unwrap()
            .expect("retry attempt should be claimed");
        assert_eq!(second.attempt, 2);
        process_redis_job_record(
            &mut connection,
            second,
            &jobs,
            &state,
            &job_admin,
            &worker_config,
        )
        .await;

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
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
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
            &job_admin,
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
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        state.job_registry().register("send_email");
        recover_stale_redis_jobs(&mut connection, &worker_b, &state, &job_admin)
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

    #[cfg(feature = "redis")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires Docker (testcontainers)"]
    async fn redis_stale_terminal_failure_keeps_retry_discard_metadata() {
        use redis::AsyncCommands as _;

        let (_container, client) = redis_test_client().await;
        let worker_a = redis_test_worker_config("autumn:test:stale-dead", "worker-a", 1);
        let worker_b = redis_test_worker_config("autumn:test:stale-dead", "worker-b", 30_000);
        redis_enqueue_test_job(&client, &worker_a, 1).await;

        let mut connection = new_redis_connection_manager(&client, "test redis worker").unwrap();
        let claimed = claim_next_redis_job(&mut connection, &worker_a)
            .await
            .unwrap()
            .expect("first worker should claim the final attempt");
        assert_eq!(claimed.claimed_by.as_deref(), Some("worker-a"));
        assert_eq!(claimed.attempt, 1);
        let failed_id = claimed.id.clone();

        tokio::time::sleep(Duration::from_millis(5)).await;
        let state = AppState::for_test().with_profile("dev");
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        state.job_registry().register("send_email");
        recover_stale_redis_jobs(&mut connection, &worker_b, &state, &job_admin)
            .await
            .unwrap();

        let dead_record_key = format!("{}{}", worker_b.dead_record_prefix, failed_id);
        let dead_record: Option<String> = connection.get(&dead_record_key).await.unwrap();
        assert!(
            dead_record.is_some(),
            "stale terminal failures need per-id metadata for admin actions"
        );
        let dead_count: usize = connection.llen(&worker_b.dead_key).await.unwrap();
        assert_eq!(dead_count, 1);

        let backend = redis_admin_test_backend(&client, &worker_b);
        backend
            .retry(&failed_id)
            .await
            .expect("stale terminal failure should be retryable from the dashboard");

        let queued_count: usize = connection.llen(&worker_b.queue_key).await.unwrap();
        let dead_count: usize = connection.llen(&worker_b.dead_key).await.unwrap();
        let dead_record_exists: bool = connection.exists(&dead_record_key).await.unwrap();
        assert_eq!(queued_count, 1);
        assert_eq!(dead_count, 0);
        assert!(!dead_record_exists);
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
            #[cfg(feature = "db")]
            pg_pool: None,
            registry: crate::actuator::JobRegistry::new(),
            job_admin: JobAdminMemoryBackend::new_for_test(32),
            default_max_attempts: 3,
            default_initial_backoff_ms: 250,
            per_job_defaults: HashMap::new(),
            interceptor: None,
        });
        assert!(global_job_client().is_some());

        clear_global_job_client();
        assert!(global_job_client().is_none());
    }

    // ── Pure-logic unit tests (no infrastructure required) ───────────────────

    #[test]
    fn job_admin_status_label_is_stable() {
        assert_eq!(JobAdminStatus::Enqueued.label(), "enqueued");
        assert_eq!(JobAdminStatus::Running.label(), "running");
        assert_eq!(JobAdminStatus::Retrying.label(), "retrying");
        assert_eq!(JobAdminStatus::Completed.label(), "completed");
        assert_eq!(JobAdminStatus::Failed.label(), "failed");
        assert_eq!(JobAdminStatus::Discarded.label(), "discarded");
        assert_eq!(JobAdminStatus::Canceled.label(), "canceled");
        assert_eq!(JobAdminStatus::Retried.label(), "retried");
    }

    #[test]
    fn job_admin_page_total_pages_rounds_up() {
        assert_eq!(JobAdminPage::new(Vec::new(), 11, 1, 5).total_pages(), 3);
        assert_eq!(JobAdminPage::new(Vec::new(), 10, 1, 5).total_pages(), 2);
        assert_eq!(JobAdminPage::new(Vec::new(), 0, 1, 5).total_pages(), 0);
        assert_eq!(JobAdminPage::new(Vec::new(), 1, 1, 5).total_pages(), 1);
    }

    #[test]
    fn job_admin_page_total_pages_is_zero_when_per_page_is_zero() {
        assert_eq!(JobAdminPage::new(Vec::new(), 5, 1, 0).total_pages(), 0);
    }

    #[test]
    fn job_admin_snapshot_empty_has_correct_shape() {
        let snap = JobAdminSnapshot::empty();
        assert_eq!(snap.enqueued.total, 0);
        assert_eq!(snap.running.total, 0);
        assert_eq!(snap.completed.total, 0);
        assert_eq!(snap.failed.total, 0);
        assert!(snap.schedules.is_empty());
        assert_eq!(snap.bounded_history_limit, DEFAULT_JOB_ADMIN_HISTORY_LIMIT);
        assert_eq!(snap.enqueued.per_page, DEFAULT_JOB_ADMIN_PER_PAGE);
    }

    #[test]
    fn job_admin_query_default_starts_at_page_one() {
        let q = JobAdminQuery::default();
        assert_eq!(q.enqueued_page, 1);
        assert_eq!(q.running_page, 1);
        assert_eq!(q.completed_page, 1);
        assert_eq!(q.failed_page, 1);
        assert_eq!(q.per_page, DEFAULT_JOB_ADMIN_PER_PAGE);
    }

    #[test]
    fn format_job_panic_extracts_owned_string_message() {
        let panic: Box<dyn std::any::Any + Send> = Box::new("stripe timed out".to_owned());
        assert_eq!(
            format_job_panic(panic.as_ref()),
            "job handler panicked: stripe timed out"
        );
    }

    #[test]
    fn format_job_panic_extracts_static_str() {
        let s: &'static str = "static panic message";
        let panic: Box<dyn std::any::Any + Send> = Box::new(s);
        assert_eq!(
            format_job_panic(panic.as_ref()),
            "job handler panicked: static panic message"
        );
    }

    #[test]
    fn format_job_panic_handles_non_string_payload() {
        let panic: Box<dyn std::any::Any + Send> = Box::new(42u32);
        assert_eq!(
            format_job_panic(panic.as_ref()),
            "job handler panicked: non-string panic payload"
        );
    }

    #[test]
    fn job_payload_identity_prefers_principal_id_over_principal_and_user_id() {
        let (principal, _) = job_payload_identity(&serde_json::json!({
            "principal_id": "pid-1",
            "principal": "pid-2",
            "user_id": 3
        }));
        assert_eq!(principal.as_deref(), Some("pid-1"));
    }

    #[test]
    fn job_payload_identity_falls_back_to_principal_then_user_id() {
        let (p1, _) = job_payload_identity(&serde_json::json!({"principal": "p-abc"}));
        assert_eq!(p1.as_deref(), Some("p-abc"));

        let (p2, _) = job_payload_identity(&serde_json::json!({"user_id": 42}));
        assert_eq!(p2.as_deref(), Some("42"));
    }

    #[test]
    fn job_payload_identity_prefers_correlation_id_over_request_id() {
        let (_, correlation) = job_payload_identity(&serde_json::json!({
            "correlation_id": "cid-1",
            "request_id": "cid-2"
        }));
        assert_eq!(correlation.as_deref(), Some("cid-1"));
    }

    #[test]
    fn job_payload_identity_falls_back_to_request_id() {
        let (_, correlation) = job_payload_identity(&serde_json::json!({"request_id": "req-abc"}));
        assert_eq!(correlation.as_deref(), Some("req-abc"));
    }

    #[test]
    fn job_payload_identity_ignores_empty_string_values() {
        let (principal, correlation) = job_payload_identity(&serde_json::json!({
            "principal_id": "",
            "user_id": 99,
            "correlation_id": "",
            "request_id": "req-fallback"
        }));
        assert_eq!(principal.as_deref(), Some("99"));
        assert_eq!(correlation.as_deref(), Some("req-fallback"));
    }

    #[test]
    fn job_payload_identity_stringifies_numeric_values() {
        let (principal, _) = job_payload_identity(&serde_json::json!({"user_id": 123}));
        assert_eq!(principal.as_deref(), Some("123"));
    }

    #[test]
    fn job_payload_identity_stringifies_boolean_values() {
        let (principal, _) = job_payload_identity(&serde_json::json!({"user_id": true}));
        assert_eq!(principal.as_deref(), Some("true"));
    }

    #[test]
    fn job_payload_identity_returns_none_for_non_object_payload() {
        let (principal, correlation) = job_payload_identity(&serde_json::json!("not an object"));
        assert!(principal.is_none());
        assert!(correlation.is_none());
    }

    #[test]
    fn job_payload_identity_returns_none_when_no_matching_keys() {
        let (principal, correlation) =
            job_payload_identity(&serde_json::json!({"unrelated": "value"}));
        assert!(principal.is_none());
        assert!(correlation.is_none());
    }

    #[test]
    fn job_admin_start_returns_missing_for_unknown_id() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        assert_eq!(
            backend.try_record_start("nonexistent", 1),
            JobAdminStartDecision::Missing
        );
    }

    #[test]
    fn job_admin_start_returns_already_transitioned_for_non_enqueued_job() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        let id = backend.record_enqueue_for_test("work", serde_json::json!({}), 1, 3);
        backend.record_start_for_test(&id, 1);
        backend.record_success_for_test(&id);

        assert_eq!(
            backend.try_record_start(&id, 1),
            JobAdminStartDecision::AlreadyTransitioned
        );
    }

    #[tokio::test]
    async fn job_admin_record_retrying_transitions_to_retrying_status() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        let id = backend.record_enqueue_for_test("work", serde_json::json!({}), 1, 3);
        backend.record_start_for_test(&id, 1);
        backend.record_retrying(&id, "temporary glitch");

        let snapshot = backend
            .snapshot(JobAdminQuery::default())
            .await
            .expect("snapshot");
        assert!(
            snapshot.running.records.is_empty(),
            "running should be empty after retrying"
        );
        assert!(
            snapshot.failed.records.is_empty(),
            "retrying is not terminal-failed"
        );
        assert!(
            snapshot.enqueued.records.is_empty(),
            "retrying is not enqueued"
        );
    }

    #[tokio::test]
    async fn job_admin_record_requeued_transitions_back_to_enqueued() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        let id = backend.record_enqueue_for_test("work", serde_json::json!({}), 1, 3);
        backend.record_start_for_test(&id, 1);
        backend.record_retrying(&id, "glitch");
        backend.record_requeued(&id, 2);

        let snapshot = backend
            .snapshot(JobAdminQuery::default())
            .await
            .expect("snapshot after requeue");
        assert_eq!(snapshot.enqueued.total, 1);
        assert_eq!(snapshot.enqueued.records[0].attempt, 2);
    }

    #[tokio::test]
    async fn job_admin_discard_rejects_non_failed_job() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        let id = backend.record_enqueue_for_test("work", serde_json::json!({}), 1, 3);

        let error = backend
            .discard(&id)
            .await
            .expect_err("enqueued job must not be discardable");
        assert!(
            error
                .to_string()
                .contains("only failed jobs can be discarded"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn job_admin_cancel_rejects_non_enqueued_job() {
        let backend = JobAdminMemoryBackend::new_for_test(32);
        let id = backend.record_enqueue_for_test("work", serde_json::json!({}), 1, 3);
        backend.record_start_for_test(&id, 1);

        let error = backend
            .cancel(&id)
            .await
            .expect_err("running job must not be cancelable");
        assert!(
            error
                .to_string()
                .contains("only enqueued jobs can be canceled"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn job_admin_history_limit_evicts_finished_jobs_keeping_active() {
        let backend = JobAdminMemoryBackend::with_history_limit(3);
        for _ in 0..3 {
            let id = backend.record_enqueue_for_test("done", serde_json::json!({}), 1, 1);
            backend.record_start_for_test(&id, 1);
            backend.record_success_for_test(&id);
        }
        let active_id = backend.record_enqueue_for_test("active", serde_json::json!({}), 1, 3);
        let overflow_id = backend.record_enqueue_for_test("overflow", serde_json::json!({}), 1, 1);
        backend.record_start_for_test(&overflow_id, 1);
        backend.record_success_for_test(&overflow_id);

        let snapshot = backend
            .snapshot(JobAdminQuery::default())
            .await
            .expect("snapshot");
        assert_eq!(
            snapshot.enqueued.total, 1,
            "active job must survive eviction"
        );
        assert_eq!(snapshot.enqueued.records[0].id, active_id);
    }

    #[tokio::test]
    async fn job_admin_snapshot_pagination_second_page() {
        let backend = JobAdminMemoryBackend::new_for_test(100);
        for i in 0..5u32 {
            backend.record_enqueue_for_test("work", serde_json::json!({"n": i}), 1, 3);
        }

        let snapshot = backend
            .snapshot(JobAdminQuery {
                enqueued_page: 2,
                running_page: 1,
                completed_page: 1,
                failed_page: 1,
                per_page: 3,
            })
            .await
            .expect("snapshot page 2");

        assert_eq!(snapshot.enqueued.total, 5);
        assert_eq!(snapshot.enqueued.records.len(), 2);
        assert_eq!(snapshot.enqueued.page, 2);
        assert_eq!(snapshot.enqueued.total_pages(), 2);
    }

    #[tokio::test]
    async fn run_job_handler_reports_async_panics() {
        let state = AppState::for_test().with_profile("dev");
        let outcome =
            run_job_handler("test_job", panicking_handler, state, serde_json::json!({})).await;
        assert_eq!(
            outcome,
            JobExecutionOutcome::Panicked("job handler panicked: forced panic".to_string())
        );
    }

    #[tokio::test]
    async fn local_unknown_job_name_records_failure_and_does_not_requeue() {
        let state = AppState::for_test().with_profile("dev");
        let jobs_by_name: Arc<RwLock<HashMap<String, JobInfo>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let (tx, mut rx) = mpsc::channel(1);
        let job_admin = JobAdminMemoryBackend::new_for_test(32);
        let job_id = job_admin.record_enqueue_for_test("ghost", serde_json::json!({}), 1, 1);

        execute_local_job(
            QueuedJob {
                id: job_id.clone(),
                name: "ghost".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 1,
                initial_backoff_ms: 1,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            },
            &jobs_by_name,
            &tx,
            &state,
            &job_admin,
        )
        .await;

        assert!(timeout(Duration::from_millis(25), rx.recv()).await.is_err());
        let snapshot = job_admin
            .snapshot(JobAdminQuery::default())
            .await
            .expect("snapshot");
        assert_eq!(snapshot.failed.total, 1);
        assert!(
            snapshot.failed.records[0]
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("unknown job")),
            "unknown job error message expected"
        );
    }

    // ── Postgres backend (RED → GREEN) ────────────────────────────────────────

    #[cfg(feature = "db")]
    mod pg {
        use super::*;
        use diesel_async::RunQueryDsl as _;

        // ── Pure-logic unit tests (no Postgres required) ──────────────────

        #[test]
        fn pg_config_default_visibility_timeout_is_thirty_seconds() {
            let config = crate::config::JobPostgresConfig::default();
            assert_eq!(config.visibility_timeout_ms, 30_000);
        }

        #[test]
        fn pg_retry_delay_grows_exponentially() {
            assert_eq!(pg_retry_delay_ms(250, 1), 250);
            assert_eq!(pg_retry_delay_ms(250, 2), 500);
            assert_eq!(pg_retry_delay_ms(250, 3), 1_000);
            assert_eq!(pg_retry_delay_ms(250, 4), 2_000);
        }

        fn pg_test_row(id: &str, name: &str, attempt: i32, max_attempts: i32) -> PgJobRow {
            PgJobRow {
                id: id.to_owned(),
                name: name.to_owned(),
                payload: "{}".to_owned(),
                status: PG_STATUS_RUNNING.to_owned(),
                attempt,
                max_attempts,
                initial_backoff_ms: 1,
                enqueued_at: None,
                started_at: None,
                finished_at: None,
                claimed_by: Some("worker".to_owned()),
                claimed_at: None,
                last_error: None,
                #[cfg(feature = "telemetry-otlp")]
                traceparent: None,
                #[cfg(feature = "telemetry-otlp")]
                tracestate: None,
            }
        }

        #[test]
        fn pg_claim_transition_requires_affected_row() {
            assert!(!pg_claim_transition_applied(0));
            assert!(pg_claim_transition_applied(1));
        }

        #[test]
        fn pg_success_lifecycle_is_skipped_when_ack_does_not_apply() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("slow_success");
            state.job_registry().record_enqueue("slow_success");
            state.job_registry().record_start("slow_success");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("slow_success", serde_json::json!({}), 1, 3);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(!record_pg_lifecycle_ack_result(
                Ok(false),
                "slow_success",
                &job_id,
                "success",
                PgLifecycleRecord::Success,
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["slow_success"].clone();
            assert_eq!(status.in_flight, 0);
            assert_eq!(status.total_successes, 0);
            let snapshot = job_admin.snapshot_sync(&JobAdminQuery::default());
            assert_eq!(snapshot.completed.total, 0);
            assert_eq!(snapshot.running.total, 0);
        }

        #[test]
        fn pg_success_lifecycle_is_recorded_after_ack_applies() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("slow_success");
            state.job_registry().record_enqueue("slow_success");
            state.job_registry().record_start("slow_success");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("slow_success", serde_json::json!({}), 1, 3);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(record_pg_lifecycle_ack_result(
                Ok(true),
                "slow_success",
                &job_id,
                "success",
                PgLifecycleRecord::Success,
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["slow_success"].clone();
            assert_eq!(status.in_flight, 0);
            assert_eq!(status.total_successes, 1);
            let snapshot = job_admin.snapshot_sync(&JobAdminQuery::default());
            assert_eq!(snapshot.completed.total, 1);
            assert_eq!(snapshot.running.total, 0);
        }

        #[test]
        fn pg_terminal_failure_stale_eviction_records_dead_letter() {
            // When a final-attempt job's ack returns Ok(false), stale-claim
            // recovery already dead-lettered the row; total_failures and
            // dead_letters must be incremented to stay in sync with the DB.
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("slow_failure");
            state.job_registry().record_enqueue("slow_failure");
            state.job_registry().record_start("slow_failure");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("slow_failure", serde_json::json!({}), 1, 1);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(!record_pg_lifecycle_ack_result(
                Ok(false),
                "slow_failure",
                &job_id,
                "failure",
                PgLifecycleRecord::Failure {
                    error: "visibility timeout expired"
                },
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["slow_failure"].clone();
            assert_eq!(status.in_flight, 0);
            assert_eq!(
                status.total_failures, 1,
                "terminal stale eviction must increment total_failures"
            );
            assert_eq!(
                status.dead_letters, 1,
                "terminal stale eviction must increment dead_letters"
            );
            let snapshot = job_admin.snapshot_sync(&JobAdminQuery::default());
            assert_eq!(
                snapshot.failed.total, 1,
                "failed list must show the dead-lettered job"
            );
            assert_eq!(snapshot.running.total, 0);
        }

        #[test]
        fn pg_failure_lifecycle_is_recorded_after_ack_applies() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("slow_failure");
            state.job_registry().record_enqueue("slow_failure");
            state.job_registry().record_start("slow_failure");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("slow_failure", serde_json::json!({}), 1, 1);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(record_pg_lifecycle_ack_result(
                Ok(true),
                "slow_failure",
                &job_id,
                "failure",
                PgLifecycleRecord::Failure {
                    error: "worker failed"
                },
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["slow_failure"].clone();
            assert_eq!(status.in_flight, 0);
            assert_eq!(status.total_failures, 1);
            assert_eq!(status.dead_letters, 1);
            let snapshot = job_admin.snapshot_sync(&JobAdminQuery::default());
            assert_eq!(snapshot.failed.total, 1);
            assert_eq!(
                snapshot.failed.records[0].last_error.as_deref(),
                Some("worker failed")
            );
        }

        #[test]
        fn pg_retry_lifecycle_is_recorded_after_ack_applies() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("slow_retry");
            state.job_registry().record_enqueue("slow_retry");
            state.job_registry().record_start("slow_retry");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("slow_retry", serde_json::json!({}), 1, 3);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(record_pg_lifecycle_ack_result(
                Ok(true),
                "slow_retry",
                &job_id,
                "failure",
                PgLifecycleRecord::Retry {
                    error: "try again",
                    attempt: 1,
                },
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["slow_retry"].clone();
            assert_eq!(status.in_flight, 0);
            assert_eq!(status.total_failures, 0);
            assert_eq!(status.last_error.as_deref(), Some("try again"));
            let admin_status = job_admin
                .inner
                .read()
                .expect("job admin lock")
                .records
                .get(&job_id)
                .expect("admin record")
                .status;
            assert_eq!(admin_status, JobAdminStatus::Enqueued);
        }

        #[test]
        fn pg_lifecycle_is_skipped_when_ack_errors() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("slow_success");
            state.job_registry().record_enqueue("slow_success");
            state.job_registry().record_start("slow_success");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("slow_success", serde_json::json!({}), 1, 3);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(!record_pg_lifecycle_ack_result(
                Err(AutumnError::internal_server_error_msg("ack failed")),
                "slow_success",
                &job_id,
                "success",
                PgLifecycleRecord::Success,
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["slow_success"].clone();
            assert_eq!(status.in_flight, 1);
            assert_eq!(status.total_successes, 0);
        }

        #[test]
        fn pg_lifecycle_balances_inflight_on_stale_eviction() {
            // When ack returns Ok(false) the claim was evicted by stale-claim
            // recovery; in_flight must be decremented so metrics don't leak.
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("evicted_job");
            state.job_registry().record_enqueue("evicted_job");
            state.job_registry().record_start("evicted_job");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("evicted_job", serde_json::json!({}), 1, 3);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(!record_pg_lifecycle_ack_result(
                Ok(false),
                "evicted_job",
                &job_id,
                "success",
                PgLifecycleRecord::Success,
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["evicted_job"].clone();
            assert_eq!(
                status.in_flight, 0,
                "in_flight must be balanced after stale eviction"
            );
            assert_eq!(status.total_successes, 0);
            assert_eq!(
                status.last_error.as_deref(),
                Some("visibility timeout expired")
            );
            let admin_status = job_admin
                .inner
                .read()
                .expect("job admin lock")
                .records
                .get(&job_id)
                .expect("admin record")
                .status;
            assert_eq!(admin_status, JobAdminStatus::Retrying);
        }

        #[test]
        fn pg_terminal_stale_eviction_records_failure_and_dead_letter() {
            // When ack returns Ok(false) on a final-attempt job (lifecycle=Failure),
            // stale recovery already dead-lettered the row in the DB.  The
            // in-memory metrics must reflect a dead-letter, not a retry.
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("terminal_job");
            state.job_registry().record_enqueue("terminal_job");
            state.job_registry().record_start("terminal_job");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("terminal_job", serde_json::json!({}), 1, 1);
            job_admin.record_start_for_test(&job_id, 1);

            assert!(!record_pg_lifecycle_ack_result(
                Ok(false),
                "terminal_job",
                &job_id,
                "failure",
                PgLifecycleRecord::Failure {
                    error: "handler timed out"
                },
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["terminal_job"].clone();
            assert_eq!(status.in_flight, 0, "in_flight must be balanced");
            assert_eq!(
                status.total_failures, 1,
                "terminal stale eviction must increment total_failures"
            );
            assert_eq!(
                status.dead_letters, 1,
                "terminal stale eviction must increment dead_letters"
            );
            let admin_status = job_admin
                .inner
                .read()
                .expect("job admin lock")
                .records
                .get(&job_id)
                .expect("admin record")
                .status;
            assert_eq!(
                admin_status,
                JobAdminStatus::Failed,
                "admin must show Failed, not Retrying, after terminal stale eviction"
            );
        }

        #[test]
        fn pg_cancel_lifecycle_waits_for_ack() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("cancel_me");
            state.job_registry().record_enqueue("cancel_me");

            assert!(!record_pg_cancel_after_ack(
                Ok(false),
                "cancel_me",
                "job-1",
                &state
            ));
            assert_eq!(state.job_registry().snapshot()["cancel_me"].queued, 1);

            assert!(record_pg_cancel_after_ack(
                Ok(true),
                "cancel_me",
                "job-1",
                &state
            ));
            assert_eq!(state.job_registry().snapshot()["cancel_me"].queued, 0);
        }

        #[test]
        fn pg_cancel_lifecycle_is_skipped_when_ack_errors() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("cancel_me");
            state.job_registry().record_enqueue("cancel_me");

            assert!(!record_pg_cancel_after_ack(
                Err(AutumnError::internal_server_error_msg("ack failed")),
                "cancel_me",
                "job-1",
                &state
            ));
            assert_eq!(state.job_registry().snapshot()["cancel_me"].queued, 1);
        }

        #[test]
        fn pg_row_lifecycle_uses_row_identity_after_ack_applies() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("row_success");
            state.job_registry().record_enqueue("row_success");
            state.job_registry().record_start("row_success");
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id =
                job_admin.record_enqueue_for_test("row_success", serde_json::json!({}), 1, 3);
            job_admin.record_start_for_test(&job_id, 1);
            let row = pg_test_row(&job_id, "row_success", 1, 3);

            assert!(record_pg_row_lifecycle_ack_result(
                Ok(true),
                &row,
                "success",
                PgLifecycleRecord::Success,
                &state,
                &job_admin
            ));

            let status = state.job_registry().snapshot()["row_success"].clone();
            assert_eq!(status.total_successes, 1);
            let snapshot = job_admin.snapshot_sync(&JobAdminQuery::default());
            assert_eq!(snapshot.completed.records[0].id, job_id);
        }

        #[test]
        fn pg_row_cancel_uses_row_identity_after_ack_applies() {
            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("row_cancel");
            state.job_registry().record_enqueue("row_cancel");
            let row = pg_test_row("row-cancel-1", "row_cancel", 1, 3);

            assert!(record_pg_row_cancel_after_ack(Ok(true), &row, &state));

            assert_eq!(state.job_registry().snapshot()["row_cancel"].queued, 0);
        }

        #[tokio::test]
        async fn pg_start_runtime_without_pool_fails_with_actionable_error() {
            let _guard = global_job_runtime_test_lock().lock().await;
            clear_global_job_client();

            let state = crate::AppState::for_test().with_profile("dev");
            let shutdown = tokio_util::sync::CancellationToken::new();
            let config = crate::config::JobConfig {
                backend: "postgres".to_string(),
                ..Default::default()
            };

            let error = start_runtime(
                vec![JobInfo {
                    name: "test_job".to_string(),
                    max_attempts: 1,
                    initial_backoff_ms: 1,
                    handler: |_state, _payload| Box::pin(async move { Ok(()) }),
                }],
                &state,
                &shutdown,
                &config,
            )
            .expect_err("postgres backend must fail when no db pool is configured");

            assert!(
                error
                    .to_string()
                    .contains("jobs.backend=postgres requires a configured database"),
                "unexpected error: {error}"
            );
            assert!(global_job_client().is_none());
            clear_global_job_client();
        }

        // ── Integration tests (Docker required) ───────────────────────────

        fn pg_test_pool(url: &str) -> PgPool {
            use diesel_async::pooled_connection::AsyncDieselConnectionManager;
            use diesel_async::pooled_connection::deadpool::Pool;
            let manager = AsyncDieselConnectionManager::<diesel_async::AsyncPgConnection>::new(url);
            Pool::builder(manager).max_size(4).build().unwrap()
        }

        async fn pg_run_migration(pool: &PgPool) {
            let mut conn = pool.get().await.unwrap();
            diesel::sql_query(include_str!(
                "../migrations/20260513000000_create_job_queue/up.sql"
            ))
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        async fn pg_exec(pool: &PgPool, sql: &str) {
            let mut conn = pool.get().await.unwrap();
            diesel::sql_query(sql).execute(&mut *conn).await.unwrap();
        }

        #[tokio::test]
        #[ignore = "requires Docker (testcontainers)"]
        async fn pg_enqueue_claim_ack_roundtrip() {
            use testcontainers::runners::AsyncRunner as _;
            use testcontainers_modules::postgres::Postgres;

            let container = Postgres::default().start().await.unwrap();
            let port = container.get_host_port_ipv4(5432).await.unwrap();
            let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

            let pool = pg_test_pool(&url);
            pg_run_migration(&pool).await;

            let job_id = uuid::Uuid::new_v4().to_string();
            pg_enqueue_job(
                &pool,
                job_id.clone(),
                "send_email",
                serde_json::json!({ "user_id": 42 }),
                5,
                250,
            )
            .await
            .expect("enqueue should succeed");

            let claimed = pg_claim_next_job(&pool, "test-worker")
                .await
                .expect("claim should return a job");

            assert_eq!(claimed.id, job_id);
            assert_eq!(claimed.name, "send_email");
            assert_eq!(claimed.status, PG_STATUS_RUNNING);
            assert_eq!(claimed.attempt, 1);
            assert_eq!(claimed.claimed_by.as_deref(), Some("test-worker"));

            pg_ack_success(&pool, &job_id, "test-worker")
                .await
                .expect("ack should succeed");

            let finished = pg_fetch_by_id(&pool, &job_id)
                .await
                .expect("job should exist after ack");
            assert_eq!(finished.status, PG_STATUS_COMPLETED);
            assert!(finished.finished_at.is_some());
            assert!(finished.claimed_by.is_none());
        }

        #[tokio::test]
        #[ignore = "requires Docker (testcontainers)"]
        async fn pg_skip_locked_prevents_double_claim_of_same_job() {
            use testcontainers::runners::AsyncRunner as _;
            use testcontainers_modules::postgres::Postgres;

            let container = Postgres::default().start().await.unwrap();
            let port = container.get_host_port_ipv4(5432).await.unwrap();
            let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

            let pool = pg_test_pool(&url);
            pg_run_migration(&pool).await;

            pg_enqueue_job(
                &pool,
                uuid::Uuid::new_v4().to_string(),
                "send_email",
                serde_json::json!({}),
                5,
                250,
            )
            .await
            .unwrap();

            let (claim_a, claim_b) = tokio::join!(
                pg_claim_next_job(&pool, "worker-a"),
                pg_claim_next_job(&pool, "worker-b")
            );

            let both = claim_a.is_some() && claim_b.is_some();
            assert!(!both, "two workers must not claim the same job");
            let one = claim_a.is_some() || claim_b.is_some();
            assert!(one, "at least one worker should claim the job");
        }

        #[tokio::test]
        #[ignore = "requires Docker (testcontainers)"]
        async fn pg_failure_retries_with_backoff_then_dead_letters() {
            use testcontainers::runners::AsyncRunner as _;
            use testcontainers_modules::postgres::Postgres;

            let container = Postgres::default().start().await.unwrap();
            let port = container.get_host_port_ipv4(5432).await.unwrap();
            let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

            let pool = pg_test_pool(&url);
            pg_run_migration(&pool).await;

            let job_id = uuid::Uuid::new_v4().to_string();
            pg_enqueue_job(&pool, job_id.clone(), "flaky", serde_json::json!({}), 2, 1)
                .await
                .unwrap();

            // Attempt 1: claim and fail
            let job = pg_claim_next_job(&pool, "worker-1")
                .await
                .expect("first claim should succeed");
            assert_eq!(job.attempt, 1);
            pg_nack_failure(&pool, &job_id, "worker-1", "first failure", &job)
                .await
                .unwrap();

            let after_first = pg_fetch_by_id(&pool, &job_id).await.unwrap();
            assert_eq!(after_first.status, PG_STATUS_ENQUEUED);
            assert_eq!(after_first.attempt, 2);

            // Fast-forward run_at so claim is immediately available
            pg_exec(
                &pool,
                &format!("UPDATE autumn_jobs SET run_at = NOW() WHERE id = '{job_id}'"),
            )
            .await;

            // Attempt 2: claim and fail again (max_attempts = 2 → dead-letter)
            let job2 = pg_claim_next_job(&pool, "worker-1")
                .await
                .expect("second claim should succeed");
            assert_eq!(job2.attempt, 2);
            pg_nack_failure(&pool, &job_id, "worker-1", "second failure", &job2)
                .await
                .unwrap();

            let final_row = pg_fetch_by_id(&pool, &job_id).await.unwrap();
            assert_eq!(final_row.status, PG_STATUS_FAILED);
            assert_eq!(final_row.last_error.as_deref(), Some("second failure"));
        }

        #[tokio::test]
        #[ignore = "requires Docker (testcontainers)"]
        async fn pg_stale_claim_requeues_within_visibility_timeout() {
            use testcontainers::runners::AsyncRunner as _;
            use testcontainers_modules::postgres::Postgres;

            let container = Postgres::default().start().await.unwrap();
            let port = container.get_host_port_ipv4(5432).await.unwrap();
            let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

            let pool = pg_test_pool(&url);
            pg_run_migration(&pool).await;

            let job_id = uuid::Uuid::new_v4().to_string();
            pg_enqueue_job(&pool, job_id.clone(), "crashy", serde_json::json!({}), 3, 1)
                .await
                .unwrap();

            let _ = pg_claim_next_job(&pool, "crashed-worker").await.unwrap();

            // Backdate claimed_at to simulate visibility timeout expiry
            pg_exec(&pool, &format!(
                "UPDATE autumn_jobs SET claimed_at = NOW() - INTERVAL '1 hour' WHERE id = '{job_id}'"
            )).await;

            // Recover stale claims with a 1-second timeout
            pg_recover_stale_claims(&pool, 1_000).await;

            let row = pg_fetch_by_id(&pool, &job_id).await.unwrap();
            assert_eq!(
                row.status, PG_STATUS_ENQUEUED,
                "stale job should be re-enqueued"
            );
            assert_eq!(row.attempt, 2, "attempt should be incremented");
            assert!(row.claimed_by.is_none(), "claim should be cleared");
            assert!(
                row.claimed_at.is_none(),
                "claim timestamp should be cleared"
            );
        }

        #[tokio::test]
        #[ignore = "requires Docker (testcontainers)"]
        async fn pg_job_admin_snapshot_returns_all_status_groups() {
            use testcontainers::runners::AsyncRunner as _;
            use testcontainers_modules::postgres::Postgres;

            let container = Postgres::default().start().await.unwrap();
            let port = container.get_host_port_ipv4(5432).await.unwrap();
            let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

            let pool = pg_test_pool(&url);
            pg_run_migration(&pool).await;

            // Enqueued
            pg_enqueue_job(
                &pool,
                "enq-1".to_string(),
                "digest",
                serde_json::json!({}),
                5,
                250,
            )
            .await
            .unwrap();

            // Running: enqueue then claim (don't ack)
            pg_enqueue_job(
                &pool,
                "run-1".to_string(),
                "reindex",
                serde_json::json!({}),
                5,
                250,
            )
            .await
            .unwrap();
            let _ = pg_claim_next_job(&pool, "w1").await;

            // Completed
            pg_enqueue_job(
                &pool,
                "cmp-1".to_string(),
                "send_email",
                serde_json::json!({}),
                5,
                250,
            )
            .await
            .unwrap();
            // claim must pick up the enqueued one (both enqueued and run-1 compete; run-1 is running)
            // so we need to force a specific claim
            pg_exec(
                &pool,
                "UPDATE autumn_jobs SET run_at = NOW() - INTERVAL '1 second' WHERE id = 'cmp-1'",
            )
            .await;
            let job_c = pg_claim_next_job(&pool, "w1")
                .await
                .expect("completed job to claim");
            pg_ack_success(&pool, &job_c.id, "w1").await.unwrap();

            // Failed
            pg_enqueue_job(
                &pool,
                "fail-1".to_string(),
                "webhook",
                serde_json::json!({}),
                1,
                1,
            )
            .await
            .unwrap();
            pg_exec(
                &pool,
                "UPDATE autumn_jobs SET run_at = NOW() - INTERVAL '1 second' WHERE id = 'fail-1'",
            )
            .await;
            let job_f = pg_claim_next_job(&pool, "w1")
                .await
                .expect("failed job to claim");
            pg_nack_failure(&pool, &job_f.id, "w1", "server down", &job_f)
                .await
                .unwrap();

            let backend = PgJobAdminBackend { pool: pool.clone() };
            let snapshot = backend.snapshot(JobAdminQuery::default()).await.unwrap();

            assert!(
                snapshot.enqueued.total >= 1,
                "expected at least one enqueued"
            );
            assert!(snapshot.running.total >= 1, "expected at least one running");
            assert!(
                snapshot.completed.total >= 1,
                "expected at least one completed"
            );
            assert!(snapshot.failed.total >= 1, "expected at least one failed");
        }

        #[tokio::test]
        #[ignore = "requires Docker (testcontainers)"]
        async fn pg_admin_retry_discard_cancel_operate_correctly() {
            use testcontainers::runners::AsyncRunner as _;
            use testcontainers_modules::postgres::Postgres;

            let container = Postgres::default().start().await.unwrap();
            let port = container.get_host_port_ipv4(5432).await.unwrap();
            let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

            let pool = pg_test_pool(&url);
            pg_run_migration(&pool).await;
            let backend = PgJobAdminBackend { pool: pool.clone() };

            // --- Retry ---
            pg_enqueue_job(
                &pool,
                "fail-r".to_string(),
                "job",
                serde_json::json!({}),
                1,
                1,
            )
            .await
            .unwrap();
            let jf = pg_claim_next_job(&pool, "w").await.unwrap();
            pg_nack_failure(&pool, &jf.id, "w", "boom", &jf)
                .await
                .unwrap();

            backend.retry("fail-r").await.expect("retry should succeed");
            let row = pg_fetch_by_id(&pool, "fail-r").await.unwrap();
            assert_eq!(row.status, PG_STATUS_ENQUEUED);
            assert_eq!(row.attempt, 1);

            // --- Discard ---
            pg_enqueue_job(
                &pool,
                "fail-d".to_string(),
                "job",
                serde_json::json!({}),
                1,
                1,
            )
            .await
            .unwrap();
            pg_exec(
                &pool,
                "UPDATE autumn_jobs SET run_at = NOW() - INTERVAL '1 second' WHERE id = 'fail-d'",
            )
            .await;
            let jd = pg_claim_next_job(&pool, "w").await.unwrap();
            pg_nack_failure(&pool, &jd.id, "w", "boom", &jd)
                .await
                .unwrap();

            backend
                .discard("fail-d")
                .await
                .expect("discard should succeed");
            let row = pg_fetch_by_id(&pool, "fail-d").await.unwrap();
            assert_eq!(row.status, "discarded");

            // --- Cancel ---
            pg_enqueue_job(
                &pool,
                "cancel-c".to_string(),
                "job",
                serde_json::json!({}),
                5,
                1,
            )
            .await
            .unwrap();
            backend
                .cancel("cancel-c")
                .await
                .expect("cancel should succeed");
            let row = pg_fetch_by_id(&pool, "cancel-c").await.unwrap();
            assert_eq!(row.status, "discarded");
        }

        /// Helper: fetch a single job row by id for test assertions.
        async fn pg_fetch_by_id(pool: &PgPool, id: &str) -> Option<PgJobRow> {
            use diesel::OptionalExtension as _;
            let mut conn = pool.get().await.unwrap();
            diesel::sql_query(format!(
                "SELECT {PG_JOB_SELECT_COLS} FROM autumn_jobs WHERE id = $1"
            ))
            .bind::<diesel::sql_types::Text, _>(id)
            .get_result::<PgJobRow>(&mut *conn)
            .await
            .optional()
            .unwrap_or(None)
        }
    }

    // ── enqueue_after_commit tests ────────────────────────────────

    fn make_test_client() -> (JobClient, tokio::sync::mpsc::Receiver<QueuedJob>) {
        let (tx, rx) = mpsc::channel(16);
        let client = JobClient {
            local_sender: Some(tx),
            #[cfg(feature = "redis")]
            redis: None,
            #[cfg(feature = "db")]
            pg_pool: None,
            registry: crate::actuator::JobRegistry::new(),
            job_admin: JobAdminMemoryBackend::new_for_test(32),
            default_max_attempts: 3,
            default_initial_backoff_ms: 100,
            per_job_defaults: HashMap::from([("test_job".to_string(), (3_u32, 100_u64))]),
            interceptor: None,
        };
        (client, rx)
    }

    #[tokio::test]
    async fn enqueue_after_commit_outside_tx_enqueues_immediately() {
        use std::time::Duration;
        let (client, mut rx) = make_test_client();

        client
            .enqueue_after_commit("test_job", serde_json::json!({"x": 1}))
            .await
            .expect("enqueue_after_commit should succeed outside tx");

        // The job should have been enqueued immediately (not deferred)
        let received = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(
            received.is_ok(),
            "job should be received immediately outside tx"
        );
        let job = received.unwrap().expect("channel should not be closed");
        assert_eq!(job.name, "test_job");
    }

    #[tokio::test]
    async fn enqueue_after_commit_inside_scope_defers_enqueue() {
        use crate::db::{AFTER_COMMIT_REGISTRY, CommitCallback};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let (client, mut rx) = make_test_client();
        let registry = Arc::new(Mutex::new(Vec::<CommitCallback>::new()));

        AFTER_COMMIT_REGISTRY
            .scope(registry.clone(), async {
                client
                    .enqueue_after_commit("test_job", serde_json::json!({"x": 2}))
                    .await
                    .expect("enqueue_after_commit should succeed inside scope");
            })
            .await;

        // Job must NOT have been enqueued yet
        let not_received = timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(
            not_received.is_err(),
            "job must not be enqueued before commit"
        );

        // Drain callbacks (simulating commit)
        let callbacks: Vec<CommitCallback> = {
            let mut reg = registry.lock().unwrap();
            std::mem::take(&mut *reg)
        };
        for cb in callbacks {
            cb().await.unwrap();
        }

        // Now the job should appear
        let received = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(
            received.is_ok(),
            "job should be enqueued after commit callbacks run"
        );
        let job = received.unwrap().expect("channel should not be closed");
        assert_eq!(job.name, "test_job");
    }

    // ── W3C Trace Context propagation tests ─────────────────────────────────

    /// Tests in this module verify the trace-context data model and helper
    /// functions introduced to propagate W3C `traceparent` / `tracestate`
    /// across job queue boundaries.  They are gated on `telemetry-otlp`
    /// because the propagation helpers and struct fields are only compiled in
    /// when that feature is enabled.
    #[cfg(feature = "telemetry-otlp")]
    mod trace_propagation {
        use super::*;

        /// Compile-time structural check: `QueuedJob` must expose
        /// `traceparent` and `tracestate` fields when `telemetry-otlp` is
        /// enabled so the in-process queue can carry the W3C context.
        #[test]
        fn queued_job_has_trace_context_fields() {
            let _job = QueuedJob {
                id: "t".to_string(),
                name: "t".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 1,
                initial_backoff_ms: 0,
                traceparent: Some(
                    "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
                ),
                tracestate: None,
            };
        }

        #[test]
        fn restore_job_trace_context_parses_valid_traceparent() {
            use opentelemetry::trace::TraceContextExt as _;
            use opentelemetry_sdk::propagation::TraceContextPropagator;

            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            let cx = restore_job_trace_context(
                Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
                None,
            )
            .expect("valid traceparent should parse into an OTel context");

            let span = cx.span();
            let sc = span.span_context();
            assert!(sc.is_valid(), "restored span context must be valid");
            assert_eq!(
                sc.trace_id().to_string(),
                "0af7651916cd43dd8448eb211c80319c",
            );
            assert_eq!(sc.span_id().to_string(), "b7ad6b7169203331");
        }

        #[test]
        fn restore_job_trace_context_returns_none_when_traceparent_absent() {
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            assert!(
                restore_job_trace_context(None, None).is_none(),
                "absent traceparent must yield None"
            );
        }

        #[test]
        fn restore_job_trace_context_returns_none_for_invalid_traceparent() {
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            assert!(
                restore_job_trace_context(Some("not-a-real-traceparent"), None).is_none(),
                "malformed traceparent must yield None"
            );
        }

        #[cfg(feature = "redis")]
        #[test]
        fn redis_record_has_trace_context_fields() {
            let _record = RedisJobRecord {
                id: "r".to_string(),
                name: "j".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 1,
                initial_backoff_ms: 0,
                enqueued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                claimed_by: None,
                claimed_at_ms: None,
                last_error: None,
                traceparent: Some(
                    "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
                ),
                tracestate: None,
            };
        }

        #[cfg(feature = "redis")]
        #[test]
        fn redis_record_missing_trace_context_deserializes_as_none() {
            let old_json = r#"{"id":"x","name":"y","payload":{},"attempt":1,"max_attempts":3,"initial_backoff_ms":250}"#;
            let record: RedisJobRecord = serde_json::from_str(old_json)
                .expect("old-format record without traceparent must deserialize");
            assert!(
                record.traceparent.is_none(),
                "missing field must default to None"
            );
            assert!(
                record.tracestate.is_none(),
                "missing field must default to None"
            );
        }

        #[cfg(feature = "telemetry-otlp")]
        #[test]
        fn job_map_injector_set_inserts_key_value() {
            use opentelemetry::propagation::Injector as _;
            let mut map = std::collections::HashMap::new();
            let mut injector = JobMapInjector(&mut map);
            injector.set(
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned(),
            );
            assert_eq!(
                map.get("traceparent").map(String::as_str),
                Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
            );
        }

        #[cfg(feature = "telemetry-otlp")]
        #[test]
        fn capture_job_trace_context_returns_none_when_no_active_span() {
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            let (tp, ts) = capture_job_trace_context();
            assert!(tp.is_none(), "no traceparent expected without active span");
            assert!(ts.is_none(), "no tracestate expected without active span");
        }

        #[cfg(feature = "telemetry-otlp")]
        #[tokio::test]
        async fn execute_local_job_with_traceparent_restores_context() {
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

            let state = AppState::for_test().with_profile("dev");
            state.job_registry().register("noop");
            state.job_registry().record_enqueue("noop");

            let mut jobs = HashMap::new();
            jobs.insert(
                "noop".to_string(),
                JobInfo {
                    name: "noop".to_string(),
                    max_attempts: 1,
                    initial_backoff_ms: 0,
                    handler: |_state, _payload| Box::pin(async { Ok(()) }),
                },
            );
            let jobs_by_name = Arc::new(RwLock::new(jobs));
            let (tx, _rx) = mpsc::channel(1);
            let job_admin = JobAdminMemoryBackend::new_for_test(32);
            let job_id = job_admin.record_enqueue_for_test("noop", serde_json::json!({}), 1, 1);

            execute_local_job(
                QueuedJob {
                    id: job_id,
                    name: "noop".to_string(),
                    payload: serde_json::json!({}),
                    attempt: 1,
                    max_attempts: 1,
                    initial_backoff_ms: 0,
                    traceparent: Some(
                        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
                    ),
                    tracestate: None,
                },
                &jobs_by_name,
                &tx,
                &state,
                &job_admin,
            )
            .await;

            let snapshot = state.job_registry().snapshot();
            assert_eq!(
                snapshot.get("noop").map(|s| s.total_successes),
                Some(1),
                "job with traceparent must execute successfully"
            );
        }

        #[cfg(feature = "redis")]
        #[test]
        fn redis_record_trace_context_survives_json_roundtrip() {
            use opentelemetry::trace::TraceContextExt as _;
            use opentelemetry_sdk::propagation::TraceContextPropagator;

            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            let original = RedisJobRecord {
                id: "r".to_string(),
                name: "j".to_string(),
                payload: serde_json::json!({}),
                attempt: 1,
                max_attempts: 1,
                initial_backoff_ms: 0,
                enqueued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                claimed_by: None,
                claimed_at_ms: None,
                last_error: None,
                traceparent: Some(
                    "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
                ),
                tracestate: None,
            };
            let encoded = serde_json::to_string(&original).expect("encode");
            let decoded: RedisJobRecord = serde_json::from_str(&encoded).expect("decode");
            let cx = restore_job_trace_context(
                decoded.traceparent.as_deref(),
                decoded.tracestate.as_deref(),
            )
            .expect("roundtrip traceparent must restore to a valid context");
            assert_eq!(
                cx.span().span_context().trace_id().to_string(),
                "0af7651916cd43dd8448eb211c80319c",
            );
        }

        #[test]
        fn job_map_extractor_keys_returns_all_keys() {
            use opentelemetry::propagation::Extractor as _;
            let mut map = std::collections::HashMap::new();
            map.insert("traceparent".to_owned(), "00-abc-def-01".to_owned());
            map.insert("tracestate".to_owned(), "vendor=val".to_owned());
            let extractor = JobMapExtractor(&map);
            let mut keys = extractor.keys();
            keys.sort_unstable();
            assert_eq!(keys, vec!["traceparent", "tracestate"]);
        }

        #[test]
        fn restore_job_trace_context_with_tracestate_parses_correctly() {
            use opentelemetry::trace::TraceContextExt as _;
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            let cx = restore_job_trace_context(
                Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
                Some("vendor=value"),
            )
            .expect("valid traceparent with tracestate should parse");
            assert!(cx.span().span_context().is_valid());
        }

        #[test]
        fn capture_job_trace_context_returns_some_when_active_otel_span() {
            use opentelemetry::trace::TracerProvider as _;
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            use opentelemetry_sdk::trace::SdkTracerProvider;
            use tracing_subscriber::prelude::*;

            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            let provider = SdkTracerProvider::builder().build();
            let tracer = provider.tracer("test");
            let sub = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(tracer));

            tracing::subscriber::with_default(sub, || {
                let span = tracing::info_span!("capture_test");
                let _guard = span.enter();
                let (tp, _ts) = capture_job_trace_context();
                assert!(
                    tp.is_some(),
                    "traceparent must be Some when an OTel-linked span is active"
                );
            });
        }

        #[test]
        fn enqueue_after_commit_span_is_included_in_queued_job() {
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let (tx, mut rx) = mpsc::channel(16);
            let client = JobClient {
                local_sender: Some(tx),
                #[cfg(feature = "redis")]
                redis: None,
                #[cfg(feature = "db")]
                pg_pool: None,
                registry: crate::actuator::JobRegistry::new(),
                job_admin: JobAdminMemoryBackend::new_for_test(32),
                default_max_attempts: 3,
                default_initial_backoff_ms: 100,
                per_job_defaults: HashMap::from([("test_job".to_string(), (3_u32, 100_u64))]),
                interceptor: None,
            };
            rt.block_on(async {
                client
                    .enqueue_after_commit("test_job", serde_json::json!({}))
                    .await
                    .expect("outside tx enqueues immediately");
                let job = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
                    .await
                    .expect("job should arrive")
                    .expect("channel open");
                assert_eq!(job.name, "test_job");
            });
        }
    }
}
