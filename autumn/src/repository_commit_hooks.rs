//! Durable dispatch for generated repository after-commit hooks.
//!
//! Generated repositories enqueue `after_*_commit` work into Postgres inside
//! the same transaction as the mutation. Any replica can later claim and run a
//! queued hook using the generated runner registered in this process.

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc, OnceLock, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use diesel::OptionalExtension as _;
use diesel_async::RunQueryDsl as _;
use diesel_async::pooled_connection::deadpool::Pool;
use futures::FutureExt as _;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{AutumnError, AutumnResult};

type PgPool = Pool<diesel_async::AsyncPgConnection>;
type HookFuture = Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>>;
type HookRunner = Arc<dyn Fn(Value, Value) -> HookFuture + Send + Sync + 'static>;

pub const REPOSITORY_COMMIT_HOOK_MIGRATIONS: diesel_migrations::EmbeddedMigrations =
    diesel_migrations::embed_migrations!("repository_commit_hook_migrations");

const HOOK_SELECT_COLS: &str = "id, handler_key, hook_name, context::TEXT AS context, \
    record::TEXT AS record, status, attempt, max_attempts, initial_backoff_ms";
const HOOK_WORKER_IDLE_SLEEP: Duration = Duration::from_millis(250);
const HOOK_STALE_CLAIM_AFTER: Duration = Duration::from_secs(60);
const HOOK_CLAIM_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const HOOK_ACK_SUCCESS_SQL: &str = "UPDATE autumn_repository_commit_hooks \
     SET status = 'completed', finished_at = NOW(), \
         context = '{}'::JSONB, record = '{}'::JSONB, \
         claimed_by = NULL, claimed_at = NULL, last_error = NULL \
     WHERE id = $1 AND claimed_by = $2 AND status = 'running'";
const HOOK_EXTEND_CLAIM_SQL: &str = "UPDATE autumn_repository_commit_hooks \
     SET claimed_at = NOW() \
     WHERE id = $1 AND claimed_by = $2 AND status = 'running'";
const HOOK_ENQUEUE_INSERT_SQL: &str = "INSERT INTO autumn_repository_commit_hooks \
     (id, handler_key, hook_name, context, record, status, attempt, \
      max_attempts, initial_backoff_ms, enqueued_at, run_at) \
     VALUES ($1, $2, $3, $4::JSONB, $5::JSONB, 'enqueued', 1, 5, 1000, NOW(), NOW())";
const HOOK_PENDING_INSERT_SQL: &str = "INSERT INTO autumn_repository_commit_hooks \
     (id, handler_key, hook_name, context, record, status, attempt, \
      max_attempts, initial_backoff_ms, enqueued_at, run_at) \
     VALUES ($1, $2, $3, $4::JSONB, $5::JSONB, 'pending_after_hook', 1, 5, 1000, NOW(), NOW())";
const HOOK_FINALIZE_AFTER_HOOK_SQL: &str = "UPDATE autumn_repository_commit_hooks \
     SET context = $1::JSONB, record = $2::JSONB, status = 'enqueued', \
         run_at = NOW(), enqueued_at = COALESCE(enqueued_at, NOW()), last_error = NULL \
     WHERE id = $3 AND status = 'pending_after_hook'";
const HOOK_DISCARD_PENDING_SQL: &str = "DELETE FROM autumn_repository_commit_hooks \
     WHERE id = $1 AND status = 'pending_after_hook'";

static REPOSITORY_COMMIT_HOOK_RUNNERS: OnceLock<
    RwLock<HashMap<String, RepositoryCommitHookRegistration>>,
> = OnceLock::new();
static REPOSITORY_COMMIT_HOOK_KICKERS: OnceLock<
    RwLock<HashMap<usize, Arc<RepositoryCommitHookKickState>>>,
> = OnceLock::new();

struct RepositoryCommitHookKickState {
    notify: Notify,
    pending: AtomicBool,
}

impl Default for RepositoryCommitHookKickState {
    fn default() -> Self {
        Self {
            notify: Notify::new(),
            pending: AtomicBool::new(false),
        }
    }
}

impl RepositoryCommitHookKickState {
    fn request_kick(&self) -> bool {
        !self.pending.swap(true, Ordering::AcqRel)
    }

    fn take_pending_kick(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
}

/// Link-time descriptor emitted by generated repositories with commit hooks.
///
/// The worker replays these descriptors at startup so queued hook rows can be
/// claimed after a process restart without waiting for request traffic to touch
/// the repository type first.
#[doc(hidden)]
pub struct RepositoryCommitHookDescriptor {
    /// Registers the generated runner for one repository type.
    pub register: fn(),
}

inventory::collect!(RepositoryCommitHookDescriptor);

#[derive(Clone)]
struct RepositoryCommitHookRegistration {
    create: HookRunner,
    update: HookRunner,
    delete: HookRunner,
}

impl RepositoryCommitHookRegistration {
    fn runner(&self, hook_name: &str) -> Option<HookRunner> {
        match hook_name {
            "create" => Some(self.create.clone()),
            "update" => Some(self.update.clone()),
            "delete" => Some(self.delete.clone()),
            _ => None,
        }
    }
}

#[derive(diesel::QueryableByName, Debug, Clone)]
#[allow(dead_code)]
struct PgRepositoryCommitHookRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    handler_key: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    hook_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    context: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    record: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    status: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    attempt: i32,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    max_attempts: i32,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    initial_backoff_ms: i64,
}

/// Register generated runners for one hooked repository type.
///
/// This is called by proc-macro-generated code and is intentionally hidden
/// behind `autumn_web::__private`.
pub fn register_repository_commit_hook_runner<
    Create,
    CreateFut,
    Update,
    UpdateFut,
    Delete,
    DeleteFut,
>(
    handler_key: &'static str,
    create: Create,
    update: Update,
    delete: Delete,
) where
    Create: Fn(Value, Value) -> CreateFut + Send + Sync + 'static,
    CreateFut: Future<Output = AutumnResult<()>> + Send + 'static,
    Update: Fn(Value, Value) -> UpdateFut + Send + Sync + 'static,
    UpdateFut: Future<Output = AutumnResult<()>> + Send + 'static,
    Delete: Fn(Value, Value) -> DeleteFut + Send + Sync + 'static,
    DeleteFut: Future<Output = AutumnResult<()>> + Send + 'static,
{
    let registration = RepositoryCommitHookRegistration {
        create: Arc::new(move |ctx, record| Box::pin(create(ctx, record))),
        update: Arc::new(move |ctx, record| Box::pin(update(ctx, record))),
        delete: Arc::new(move |ctx, record| Box::pin(delete(ctx, record))),
    };

    REPOSITORY_COMMIT_HOOK_RUNNERS
        .get_or_init(|| RwLock::new(HashMap::new()))
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(handler_key.to_owned(), registration);
}

fn register_inventory_repository_commit_hook_runners() {
    for descriptor in inventory::iter::<RepositoryCommitHookDescriptor> {
        (descriptor.register)();
    }
}

pub fn has_repository_commit_hook_descriptors() -> bool {
    inventory::iter::<RepositoryCommitHookDescriptor>
        .into_iter()
        .next()
        .is_some()
}

/// Insert a generated repository commit hook row using the caller's open
/// connection. The row participates in the caller's transaction.
///
/// # Errors
///
/// Returns an error when the context or record cannot be serialized, or when
/// Postgres rejects the enqueue insert.
pub async fn enqueue_repository_commit_hook_on_conn<C, R>(
    conn: &mut diesel_async::AsyncPgConnection,
    handler_key: &str,
    hook_name: &str,
    context: &C,
    record: &R,
) -> AutumnResult<()>
where
    C: Serialize + Sync + ?Sized,
    R: Serialize + Sync + ?Sized,
{
    let (context, record) = serialize_repository_commit_hook_payloads(context, record)?;
    let id = uuid::Uuid::new_v4().to_string();

    diesel::sql_query(HOOK_ENQUEUE_INSERT_SQL)
        .bind::<diesel::sql_types::Text, _>(id)
        .bind::<diesel::sql_types::Text, _>(handler_key)
        .bind::<diesel::sql_types::Text, _>(hook_name)
        .bind::<diesel::sql_types::Text, _>(context)
        .bind::<diesel::sql_types::Text, _>(record)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook enqueue failed: {error}"
            ))
        })
}

/// Insert a generated repository commit hook row in a staged state.
///
/// The row participates in the caller's transaction but cannot be claimed by a
/// dispatcher until [`finalize_repository_commit_hook_after_hook`] promotes it
/// after the regular `after_*` hook has succeeded.
///
/// # Errors
///
/// Returns an error when the context or record cannot be serialized, or when
/// Postgres rejects the staged insert.
pub async fn enqueue_repository_commit_hook_pending_on_conn<C, R>(
    conn: &mut diesel_async::AsyncPgConnection,
    handler_key: &str,
    hook_name: &str,
    context: &C,
    record: &R,
) -> AutumnResult<String>
where
    C: Serialize + Sync + ?Sized,
    R: Serialize + Sync + ?Sized,
{
    let (context, record) = serialize_repository_commit_hook_payloads(context, record)?;
    let id = uuid::Uuid::new_v4().to_string();

    diesel::sql_query(HOOK_PENDING_INSERT_SQL)
        .bind::<diesel::sql_types::Text, _>(id.clone())
        .bind::<diesel::sql_types::Text, _>(handler_key)
        .bind::<diesel::sql_types::Text, _>(hook_name)
        .bind::<diesel::sql_types::Text, _>(context)
        .bind::<diesel::sql_types::Text, _>(record)
        .execute(conn)
        .await
        .map(|_| id)
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook staging failed: {error}"
            ))
        })
}

/// Promote a staged create/update commit hook after the regular after hook
/// succeeds, rewriting the row with the finalized mutation context.
///
/// # Errors
///
/// Returns an error when serialization fails, the database cannot be reached,
/// or the staged row is no longer present.
pub async fn finalize_repository_commit_hook_after_hook<C, R>(
    pool: &PgPool,
    hook_id: &str,
    context: &C,
    record: &R,
) -> AutumnResult<()>
where
    C: Serialize + Sync + ?Sized,
    R: Serialize + Sync + ?Sized,
{
    let (context, record) = serialize_repository_commit_hook_payloads(context, record)?;
    let mut conn = pool.get().await.map_err(|error| {
        AutumnError::internal_server_error_msg(format!("pg pool error: {error}"))
    })?;

    let rows = diesel::sql_query(HOOK_FINALIZE_AFTER_HOOK_SQL)
        .bind::<diesel::sql_types::Text, _>(context)
        .bind::<diesel::sql_types::Text, _>(record)
        .bind::<diesel::sql_types::Text, _>(hook_id)
        .execute(&mut *conn)
        .await
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook finalization failed: {error}"
            ))
        })?;

    if rows > 0 {
        Ok(())
    } else {
        Err(AutumnError::internal_server_error_msg(format!(
            "repository commit hook finalization skipped missing staged row: {hook_id}"
        )))
    }
}

/// Discard a staged create/update commit hook after the regular after hook
/// fails. This preserves the previous lifecycle: after-commit work is only
/// registered after the regular after hook succeeds.
///
/// # Errors
///
/// Returns an error when the database cannot be reached or rejects the delete.
pub async fn discard_repository_commit_hook_pending(
    pool: &PgPool,
    hook_id: &str,
) -> AutumnResult<()> {
    let mut conn = pool.get().await.map_err(|error| {
        AutumnError::internal_server_error_msg(format!("pg pool error: {error}"))
    })?;

    diesel::sql_query(HOOK_DISCARD_PENDING_SQL)
        .bind::<diesel::sql_types::Text, _>(hook_id)
        .execute(&mut *conn)
        .await
        .map(|_| ())
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook pending discard failed: {error}"
            ))
        })
}

fn serialize_repository_commit_hook_payloads<C, R>(
    context: &C,
    record: &R,
) -> AutumnResult<(String, String)>
where
    C: Serialize + ?Sized,
    R: Serialize + ?Sized,
{
    let context = serde_json::to_string(context).map_err(|error| {
        AutumnError::internal_server_error_msg(format!(
            "serialize repository commit hook context: {error}"
        ))
    })?;
    let record = serde_json::to_string(record).map_err(|error| {
        AutumnError::internal_server_error_msg(format!(
            "serialize repository commit hook record: {error}"
        ))
    })?;

    Ok((context, record))
}

pub fn start_repository_commit_hook_worker(pool: PgPool, shutdown: CancellationToken) {
    register_inventory_repository_commit_hook_runners();
    if !should_start_repository_commit_hook_worker(&registered_handler_keys()) {
        return;
    }

    let worker_id = repository_commit_hook_worker_id();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(HOOK_WORKER_IDLE_SLEEP) => {
                    recover_stale_repository_commit_hooks(&pool, &worker_id).await;
                    drain_ready_repository_commit_hooks(&pool, &worker_id, 32).await;
                }
            }
        }
    });
}

/// Nudge dispatch after a mutation commits, without relying on this replica for
/// durability. Polling workers on all replicas can still claim the row later.
pub fn kick_repository_commit_hook_dispatcher(pool: &PgPool) {
    register_inventory_repository_commit_hook_runners();
    if !should_start_repository_commit_hook_worker(&registered_handler_keys()) {
        return;
    }

    let state = repository_commit_hook_kick_state(pool);
    if state.request_kick() {
        state.notify.notify_one();
    }
}

fn repository_commit_hook_kick_state(pool: &PgPool) -> Arc<RepositoryCommitHookKickState> {
    let key = repository_commit_hook_pool_key(pool);
    let registry = REPOSITORY_COMMIT_HOOK_KICKERS.get_or_init(|| RwLock::new(HashMap::new()));
    let existing = {
        let registry = registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry.get(&key).cloned()
    };
    if let Some(state) = existing {
        return state;
    }

    let mut registry = registry
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(state) = registry.get(&key).cloned() {
        return state;
    }

    let state = Arc::new(RepositoryCommitHookKickState::default());
    spawn_repository_commit_hook_kick_worker(pool.clone(), state.clone());
    registry.insert(key, state.clone());
    state
}

fn repository_commit_hook_pool_key(pool: &PgPool) -> usize {
    std::ptr::from_ref(pool.manager()).addr()
}

fn spawn_repository_commit_hook_kick_worker(
    pool: PgPool,
    state: Arc<RepositoryCommitHookKickState>,
) {
    let worker_id = repository_commit_hook_worker_id();
    tokio::spawn(async move {
        loop {
            state.notify.notified().await;
            while state.take_pending_kick() {
                drain_ready_repository_commit_hooks(&pool, &worker_id, 32).await;
            }
        }
    });
}

async fn drain_ready_repository_commit_hooks(pool: &PgPool, worker_id: &str, max_rows: usize) {
    for _ in 0..max_rows {
        let Some(row) = pg_claim_next_repository_commit_hook(pool, worker_id).await else {
            break;
        };

        let heartbeat_shutdown = CancellationToken::new();
        let heartbeat_task = tokio::spawn(heartbeat_repository_commit_hook_claim(
            pool.clone(),
            row.id.clone(),
            worker_id.to_owned(),
            heartbeat_shutdown.child_token(),
        ));
        let result = run_repository_commit_hook_row(&row).await;

        match result {
            Ok(()) => {
                if let Err(error) =
                    pg_ack_repository_commit_hook_success(pool, &row.id, worker_id).await
                {
                    tracing::warn!(
                        hook_id = %row.id,
                        error = %error,
                        "failed to ack repository commit hook success"
                    );
                }
            }
            Err(error) => {
                let failures_total = crate::db::record_after_commit_failure();
                tracing::error!(
                    hook_id = %row.id,
                    handler_key = %row.handler_key,
                    hook_name = %row.hook_name,
                    autumn.after_commit.failures_total = failures_total,
                    "repository after_commit hook failed: {error}"
                );
                if let Err(nack_error) =
                    pg_nack_repository_commit_hook_failure(pool, &row.id, worker_id, &error, &row)
                        .await
                {
                    tracing::warn!(
                        hook_id = %row.id,
                        error = %nack_error,
                        "failed to record repository commit hook failure"
                    );
                }
            }
        }

        heartbeat_shutdown.cancel();
        if let Err(error) = heartbeat_task.await {
            tracing::warn!(
                hook_id = %row.id,
                error = %error,
                "repository commit hook heartbeat task failed"
            );
        }
    }
}

async fn run_repository_commit_hook_row(row: &PgRepositoryCommitHookRow) -> Result<(), String> {
    let context = serde_json::from_str::<Value>(&row.context)
        .map_err(|error| format!("decode repository hook context: {error}"))?;
    let record = serde_json::from_str::<Value>(&row.record)
        .map_err(|error| format!("decode repository hook record: {error}"))?;
    let result = std::panic::AssertUnwindSafe(run_registered_repository_commit_hook(
        &row.handler_key,
        &row.hook_name,
        context,
        record,
    ))
    .catch_unwind()
    .await;

    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(panic) => Err(format_repository_commit_hook_panic(&*panic)),
    }
}

async fn run_registered_repository_commit_hook(
    handler_key: &str,
    hook_name: &str,
    context: Value,
    record: Value,
) -> AutumnResult<()> {
    let runner = {
        let registry = REPOSITORY_COMMIT_HOOK_RUNNERS
            .get_or_init(|| RwLock::new(HashMap::new()))
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry
            .get(handler_key)
            .and_then(|registration| registration.runner(hook_name))
    };

    let Some(runner) = runner else {
        return Err(AutumnError::internal_server_error_msg(format!(
            "repository commit hook runner not registered: handler_key={handler_key}, hook_name={hook_name}"
        )));
    };

    runner(context, record).await
}

async fn heartbeat_repository_commit_hook_claim(
    pool: PgPool,
    hook_id: String,
    worker_id: String,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = tokio::time::sleep(HOOK_CLAIM_HEARTBEAT_INTERVAL) => {
                match pg_extend_repository_commit_hook_claim(&pool, &hook_id, &worker_id).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        tracing::warn!(
                            hook_id = %hook_id,
                            error = %error,
                            "failed to extend repository commit hook claim"
                        );
                    }
                }
            }
        }
    }
}

fn registered_handler_keys() -> Vec<String> {
    REPOSITORY_COMMIT_HOOK_RUNNERS
        .get_or_init(|| RwLock::new(HashMap::new()))
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keys()
        .cloned()
        .collect()
}

const fn should_start_repository_commit_hook_worker(handler_keys: &[String]) -> bool {
    !handler_keys.is_empty()
}

async fn pg_claim_next_repository_commit_hook(
    pool: &PgPool,
    worker_id: &str,
) -> Option<PgRepositoryCommitHookRow> {
    let handler_keys = registered_handler_keys();
    if handler_keys.is_empty() {
        return None;
    }

    let mut conn = pool.get().await.ok()?;
    let sql = format!(
        "UPDATE autumn_repository_commit_hooks \
         SET status = 'running', started_at = NOW(), claimed_by = $2, claimed_at = NOW() \
         WHERE id = ( \
           SELECT id FROM autumn_repository_commit_hooks \
           WHERE status = 'enqueued' \
             AND run_at <= NOW() \
             AND handler_key = ANY($1) \
           ORDER BY run_at ASC, enqueued_at ASC \
           LIMIT 1 \
           FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING {HOOK_SELECT_COLS}"
    );

    diesel::sql_query(sql)
        .bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(handler_keys)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .get_result::<PgRepositoryCommitHookRow>(&mut *conn)
        .await
        .optional()
        .unwrap_or_else(|error| {
            if is_missing_hook_table_error(&error) {
                tracing::debug!(
                    error = %error,
                    "repository commit hook queue table is not available yet"
                );
            } else {
                tracing::warn!(error = %error, "repository commit hook claim query failed");
            }
            None
        })
}

async fn pg_ack_repository_commit_hook_success(
    pool: &PgPool,
    hook_id: &str,
    worker_id: &str,
) -> AutumnResult<bool> {
    let mut conn = pool.get().await.map_err(|error| {
        AutumnError::internal_server_error_msg(format!("pg pool error: {error}"))
    })?;

    diesel::sql_query(HOOK_ACK_SUCCESS_SQL)
        .bind::<diesel::sql_types::Text, _>(hook_id)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .execute(&mut *conn)
        .await
        .map(|rows| rows > 0)
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook ack failed: {error}"
            ))
        })
}

async fn pg_extend_repository_commit_hook_claim(
    pool: &PgPool,
    hook_id: &str,
    worker_id: &str,
) -> AutumnResult<bool> {
    let mut conn = pool.get().await.map_err(|error| {
        AutumnError::internal_server_error_msg(format!("pg pool error: {error}"))
    })?;

    diesel::sql_query(HOOK_EXTEND_CLAIM_SQL)
        .bind::<diesel::sql_types::Text, _>(hook_id)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .execute(&mut *conn)
        .await
        .map(|rows| rows > 0)
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook claim heartbeat failed: {error}"
            ))
        })
}

async fn pg_nack_repository_commit_hook_failure(
    pool: &PgPool,
    hook_id: &str,
    worker_id: &str,
    error: &str,
    row: &PgRepositoryCommitHookRow,
) -> AutumnResult<bool> {
    let mut conn = pool.get().await.map_err(|error| {
        AutumnError::internal_server_error_msg(format!("pg pool error: {error}"))
    })?;

    if row.attempt < row.max_attempts {
        let delay_ms = retry_delay_ms(row.initial_backoff_ms, row.attempt);
        diesel::sql_query(
            "UPDATE autumn_repository_commit_hooks \
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
        .bind::<diesel::sql_types::Text, _>(hook_id)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .execute(&mut *conn)
        .await
        .map(|rows| rows > 0)
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook retry failed: {error}"
            ))
        })
    } else {
        diesel::sql_query(
            "UPDATE autumn_repository_commit_hooks \
             SET status = 'failed', \
                 finished_at = NOW(), \
                 claimed_by = NULL, \
                 claimed_at = NULL, \
                 last_error = $1 \
             WHERE id = $2 AND claimed_by = $3 AND status = 'running'",
        )
        .bind::<diesel::sql_types::Text, _>(error)
        .bind::<diesel::sql_types::Text, _>(hook_id)
        .bind::<diesel::sql_types::Text, _>(worker_id)
        .execute(&mut *conn)
        .await
        .map(|rows| rows > 0)
        .map_err(|error| {
            AutumnError::internal_server_error_msg(format!(
                "repository commit hook dead-letter failed: {error}"
            ))
        })
    }
}

async fn recover_stale_repository_commit_hooks(pool: &PgPool, worker_id: &str) {
    let stale_after_ms = i64::try_from(HOOK_STALE_CLAIM_AFTER.as_millis()).unwrap_or(i64::MAX);
    let Ok(mut conn) = pool.get().await else {
        return;
    };

    if let Err(error) = diesel::sql_query(
        "UPDATE autumn_repository_commit_hooks \
         SET status = 'enqueued', \
             run_at = NOW(), \
             started_at = NULL, \
             claimed_by = NULL, \
             claimed_at = NULL, \
             last_error = COALESCE(last_error, $1) \
         WHERE status = 'running' \
           AND claimed_at < NOW() - ($2::BIGINT * INTERVAL '1 millisecond')",
    )
    .bind::<diesel::sql_types::Text, _>(format!("stale claim recovered by {worker_id}"))
    .bind::<diesel::sql_types::BigInt, _>(stale_after_ms)
    .execute(&mut *conn)
    .await
    {
        if is_missing_hook_table_error(&error) {
            tracing::debug!(
                error = %error,
                "repository commit hook queue table is not available yet"
            );
        } else {
            tracing::warn!(error = %error, "repository commit hook stale recovery failed");
        }
    }
}

fn is_missing_hook_table_error(error: &diesel::result::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("autumn_repository_commit_hooks")
        && (message.contains("does not exist") || message.contains("undefinedtable"))
}

fn retry_delay_ms(initial_backoff_ms: i64, attempt: i32) -> i64 {
    let exp = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0);
    initial_backoff_ms.saturating_mul(2_i64.saturating_pow(exp))
}

fn format_repository_commit_hook_panic(payload: &(dyn Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));

    message.map_or_else(
        || "repository commit hook panicked".to_owned(),
        |message| format!("repository commit hook panicked: {message}"),
    )
}

fn repository_commit_hook_worker_id() -> String {
    format!("repository-hook-{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn registered_runner_executes_matching_hook() {
        let calls = Arc::new(AtomicUsize::new(0));
        let create_calls = calls.clone();
        let handler_key: &'static str = Box::leak(
            format!(
                "test::registered_runner_executes_matching_hook::{}",
                uuid::Uuid::new_v4()
            )
            .into_boxed_str(),
        );

        register_repository_commit_hook_runner(
            handler_key,
            move |_ctx, _record| {
                let create_calls = create_calls.clone();
                async move {
                    create_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            |_ctx, _record| async { Ok(()) },
            |_ctx, _record| async { Ok(()) },
        );

        run_registered_repository_commit_hook(handler_key, "create", Value::Null, Value::Null)
            .await
            .unwrap();

        assert_eq!(calls.as_ref().load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_runner_returns_recoverable_error() {
        let err = run_registered_repository_commit_hook(
            "missing-handler",
            "create",
            Value::Null,
            Value::Null,
        )
        .await
        .expect_err("missing runner should be reported");

        assert!(
            err.to_string().contains("runner not registered"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn worker_start_is_disabled_without_registered_handlers() {
        assert!(
            !should_start_repository_commit_hook_worker(&[]),
            "unhooked DB apps must not poll the hook queue"
        );
    }

    #[test]
    fn worker_start_is_enabled_with_registered_handlers() {
        assert!(
            should_start_repository_commit_hook_worker(&["handler".to_owned()]),
            "hooked DB apps should poll the hook queue"
        );
    }

    #[test]
    fn dispatcher_kick_state_coalesces_pending_notifications() {
        let state = RepositoryCommitHookKickState::default();

        assert!(state.request_kick(), "first kick should notify the worker");
        assert!(
            !state.request_kick(),
            "repeated kicks while one is pending must coalesce"
        );
        assert!(
            state.take_pending_kick(),
            "worker should observe one wakeup"
        );
        assert!(
            !state.take_pending_kick(),
            "observed wakeup should clear pending state"
        );
        assert!(
            state.request_kick(),
            "a later kick after the worker drains should notify again"
        );
    }

    #[test]
    fn retry_delay_is_exponential() {
        assert_eq!(retry_delay_ms(100, 1), 100);
        assert_eq!(retry_delay_ms(100, 2), 200);
        assert_eq!(retry_delay_ms(100, 3), 400);
    }

    #[test]
    fn claim_heartbeat_runs_before_stale_recovery() {
        assert!(
            HOOK_CLAIM_HEARTBEAT_INTERVAL < HOOK_STALE_CLAIM_AFTER,
            "heartbeat interval must be shorter than stale recovery threshold"
        );
    }

    #[test]
    fn success_ack_clears_retained_payloads() {
        assert!(
            HOOK_ACK_SUCCESS_SQL.contains("context = '{}'::JSONB"),
            "success ack must clear serialized context payload"
        );
        assert!(
            HOOK_ACK_SUCCESS_SQL.contains("record = '{}'::JSONB"),
            "success ack must clear serialized record payload"
        );
    }

    #[test]
    fn staged_hooks_are_not_dispatchable_until_finalized_after_regular_hooks() {
        assert!(
            HOOK_PENDING_INSERT_SQL.contains("status, attempt")
                && HOOK_PENDING_INSERT_SQL.contains("'pending_after_hook'"),
            "create/update hooks must first be staged in a non-dispatchable lifecycle state"
        );
        assert!(
            HOOK_FINALIZE_AFTER_HOOK_SQL.contains("status = 'enqueued'"),
            "after-hook finalization must make the row dispatchable only after regular hooks succeed"
        );
        assert!(
            HOOK_FINALIZE_AFTER_HOOK_SQL
                .contains("WHERE id = $3 AND status = 'pending_after_hook'"),
            "finalization must only promote the staged row it owns"
        );
        assert!(
            HOOK_DISCARD_PENDING_SQL.contains("DELETE FROM autumn_repository_commit_hooks")
                && HOOK_DISCARD_PENDING_SQL.contains("status = 'pending_after_hook'"),
            "failed regular after hooks must discard the staged commit-hook row"
        );
    }

    #[test]
    fn claim_heartbeat_is_owner_scoped() {
        assert!(
            HOOK_EXTEND_CLAIM_SQL.contains("claimed_at = NOW()"),
            "heartbeat must extend the stale-recovery lease"
        );
        assert!(
            HOOK_EXTEND_CLAIM_SQL.contains("claimed_by = $2"),
            "heartbeat must only extend this worker's claim"
        );
        assert!(
            HOOK_EXTEND_CLAIM_SQL.contains("status = 'running'"),
            "heartbeat must only touch running rows"
        );
    }

    #[test]
    fn missing_hook_table_error_is_detected_for_quiet_polling() {
        let error = diesel::result::Error::QueryBuilderError(
            std::io::Error::other("relation \"autumn_repository_commit_hooks\" does not exist")
                .into(),
        );

        assert!(is_missing_hook_table_error(&error));
    }
}
