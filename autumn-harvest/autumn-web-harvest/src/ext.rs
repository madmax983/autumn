//! Autumn `AppBuilder` integration for Harvest worker lifecycle.

use std::any::Any;
use std::sync::{Arc, Mutex};

use autumn_web::AppBuilder;
use autumn_web::AppState;
use autumn_web::config::AutumnConfig;
use autumn_web::config::DatabaseConfig;
use autumn_web::create_pool;
use autumn_web::error::AutumnError;
use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::api::{HarvestApiState, harvest_api_router};
use crate::config::{HarvestMode, HarvestRuntimeConfig};
use crate::outbox::spawn_workflow_start_outbox_relay;
use crate::runner::{HarvestRunner, HarvestRunnerResources};
use autumn_harvest::builder::{HarvestBuilder, WorkerConfig};
use autumn_harvest::info::{ActivityInfo, DagInfo, WorkflowInfo};
use autumn_harvest::worker::DbPool;

const HARVEST_MIGRATIONS: EmbeddedMigrations = embed_migrations!("../autumn-harvest/migrations");
const OUTBOX_MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

struct OutboxRuntime {
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

struct HarvestRuntime {
    runner: HarvestRunner,
    outbox: Option<OutboxRuntime>,
}

#[derive(Default)]
struct HarvestRegistration {
    builder: HarvestBuilder,
    api_path: Option<String>,
}

#[derive(Default)]
struct HarvestIntegrationShared {
    registration: HarvestRegistration,
    runtime: Option<HarvestRuntime>,
}

struct HarvestIntegration {
    shared: Arc<Mutex<HarvestIntegrationShared>>,
    api_state: HarvestApiState,
    hooks_registered: bool,
    api_route_registered: bool,
}

impl Default for HarvestIntegration {
    fn default() -> Self {
        Self {
            shared: Arc::new(Mutex::new(HarvestIntegrationShared::default())),
            api_state: HarvestApiState::new(),
            hooks_registered: false,
            api_route_registered: false,
        }
    }
}

/// Extension trait embedding Harvest into Autumn's application lifecycle.
///
/// The registered Harvest worker starts after [`AppState`] is constructed and
/// stops during graceful shutdown. Harvest migrations are appended to the
/// app's migration set the first time one of these methods is used.
pub trait HarvestExt {
    /// Register workflow definitions produced by `autumn_harvest::workflows!`.
    #[must_use]
    fn workflows(self, workflows: Vec<WorkflowInfo>) -> Self;

    /// Register activity definitions produced by `autumn_harvest::activities!`.
    #[must_use]
    fn activities(self, activities: Vec<ActivityInfo>) -> Self;

    /// Register DAG definitions produced by `autumn_harvest::dags!`.
    #[must_use]
    fn dags(self, dags: Vec<DagInfo>) -> Self;

    /// Register typed shared state visible to workflow and activity handlers.
    #[must_use]
    fn state<T: Any + Send + Sync>(self, value: T) -> Self;

    /// Configure the worker runtime.
    #[must_use]
    fn worker(self, config: WorkerConfig) -> Self;

    /// Mount the Harvest management API under `path`.
    #[must_use]
    fn harvest_api(self, path: &str) -> Self;
}

impl HarvestExt for AppBuilder {
    fn workflows(self, workflows: Vec<WorkflowInfo>) -> Self {
        configure_harvest(self, move |registration| {
            registration.builder = std::mem::take(&mut registration.builder).workflows(workflows);
        })
    }

    fn activities(self, activities: Vec<ActivityInfo>) -> Self {
        configure_harvest(self, move |registration| {
            registration.builder = std::mem::take(&mut registration.builder).activities(activities);
        })
    }

    fn dags(self, dags: Vec<DagInfo>) -> Self {
        configure_harvest(self, move |registration| {
            registration.builder = std::mem::take(&mut registration.builder).dags(dags);
        })
    }

    fn state<T: Any + Send + Sync>(self, value: T) -> Self {
        configure_harvest(self, move |registration| {
            registration.builder = std::mem::take(&mut registration.builder).state(value);
        })
    }

    fn worker(self, config: WorkerConfig) -> Self {
        configure_harvest(self, move |registration| {
            registration.builder = std::mem::take(&mut registration.builder).worker(config);
        })
    }

    fn harvest_api(self, path: &str) -> Self {
        let path = path.to_owned();
        configure_harvest(self, move |registration| {
            registration.api_path = Some(path);
        })
    }
}

fn configure_harvest<F>(builder: AppBuilder, update: F) -> AppBuilder
where
    F: FnOnce(&mut HarvestRegistration),
{
    let mut register_hooks = false;
    let mut api_mount = None;
    let builder = builder.update_extension::<HarvestIntegration, _, _>(
        HarvestIntegration::default,
        |integration| {
            {
                let mut shared = integration.shared.lock().expect("harvest lock poisoned");
                update(&mut shared.registration);
                if !integration.api_route_registered {
                    if let Some(path) = shared.registration.api_path.clone() {
                        integration.api_route_registered = true;
                        api_mount = Some((path, integration.api_state.clone()));
                    }
                }
            }

            if !integration.hooks_registered {
                integration.hooks_registered = true;
                register_hooks = true;
            }
        },
    );

    if !register_hooks {
        return if let Some((path, api_state)) = api_mount {
            builder.nest(&path, harvest_api_router(api_state))
        } else {
            builder
        };
    }

    let integration = builder
        .extension::<HarvestIntegration>()
        .expect("harvest integration should be present");
    let shared = integration.shared.clone();
    let api_state = integration.api_state.clone();
    let startup_shared = Arc::clone(&shared);
    let shutdown_shared = Arc::clone(&shared);
    let startup_api_state = api_state.clone();
    let shutdown_api_state = api_state;

    let builder = builder
        .on_startup(move |state| {
            let shared = Arc::clone(&startup_shared);
            let api_state = startup_api_state.clone();
            async move { start_harvest_runtime(&state, &shared, &api_state) }
        })
        .on_shutdown(move || {
            let shared = Arc::clone(&shutdown_shared);
            let api_state = shutdown_api_state.clone();
            async move {
                stop_harvest_runtime(shared, api_state).await;
            }
        });

    if let Some((path, api_state)) = api_mount {
        builder.nest(&path, harvest_api_router(api_state))
    } else {
        builder
    }
}

fn start_harvest_runtime(
    state: &AppState,
    shared: &Arc<Mutex<HarvestIntegrationShared>>,
    api_state: &HarvestApiState,
) -> autumn_web::AutumnResult<()> {
    let app_config = AutumnConfig::load()
        .map_err(|error| AutumnError::service_unavailable_msg(error.to_string()))?;
    let harvest_config = HarvestRuntimeConfig::load()
        .map_err(|error| AutumnError::service_unavailable_msg(error.to_string()))?;
    ensure_runtime_migrations(state.profile(), &app_config, &harvest_config)?;

    let runtime_state = state.clone();
    let app_pool = state.pool().cloned();
    let harvest_pool = resolve_harvest_pool(state, &harvest_config)?;

    let (registration, runtime_already_started) = {
        let mut guard = shared.lock().expect("harvest lock poisoned");
        (
            std::mem::take(&mut guard.registration),
            guard.runtime.is_some(),
        )
    };

    if runtime_already_started {
        tracing::warn!("harvest runtime already started; skipping duplicate startup");
        return Ok(());
    }

    let built = registration.builder.build();
    state.insert_extension(harvest_config.outbox.clone());
    let mut runner_resources =
        HarvestRunnerResources::new(harvest_pool).with_app_state(runtime_state.clone());
    if let Some(app_pool) = app_pool.as_ref() {
        runner_resources = runner_resources.with_app_pool(app_pool.clone());
    }
    let runner = HarvestRunner::start(built, &harvest_config, runner_resources)?;
    let harvest_db_pool = runner.storage_pool();
    state.insert_extension(harvest_db_pool.clone());
    api_state.install_storage_pool(harvest_db_pool);
    let outbox = app_pool.as_ref().and_then(|_| {
        if harvest_config.outbox.enabled {
            let shutdown = CancellationToken::new();
            let handle =
                spawn_workflow_start_outbox_relay(runtime_state.clone(), shutdown.child_token());
            Some(OutboxRuntime { shutdown, handle })
        } else {
            None
        }
    });
    api_state.install(runner.api_runtime());

    {
        let mut guard = shared.lock().expect("harvest lock poisoned");
        guard.runtime = Some(HarvestRuntime { runner, outbox });
    }
    Ok(())
}

fn resolve_harvest_pool(
    state: &AppState,
    config: &HarvestRuntimeConfig,
) -> autumn_web::AutumnResult<DbPool> {
    match config.mode {
        HarvestMode::Embedded => state.pool().cloned().ok_or_else(|| {
            AutumnError::service_unavailable_msg("autumn-harvest requires a configured database")
        }),
        HarvestMode::Split | HarvestMode::External => {
            let database = DatabaseConfig {
                url: config.database.url.clone(),
                ..DatabaseConfig::default()
            };
            create_pool(&database)
                .map_err(|error| AutumnError::service_unavailable_msg(error.to_string()))?
                .ok_or_else(|| {
                    AutumnError::service_unavailable_msg(
                        "harvest.database.url must resolve to a dedicated database pool",
                    )
                })
        }
    }
}

async fn stop_harvest_runtime(
    shared: Arc<Mutex<HarvestIntegrationShared>>,
    api_state: HarvestApiState,
) {
    let runtime = { shared.lock().expect("harvest lock poisoned").runtime.take() };

    let Some(runtime) = runtime else {
        api_state.clear();
        return;
    };

    if let Some(outbox) = runtime.outbox {
        outbox.shutdown.cancel();
        if let Err(error) = outbox.handle.await {
            if !error.is_cancelled() {
                tracing::warn!(error = %error, "harvest outbox relay failed during shutdown");
            }
        }
    }
    runtime.runner.stop().await;
    api_state.clear();
}

fn ensure_runtime_migrations(
    profile: &str,
    app_config: &AutumnConfig,
    harvest_config: &HarvestRuntimeConfig,
) -> autumn_web::AutumnResult<()> {
    if let Some(app_database_url) = app_config.database.url.as_deref() {
        apply_migrations_for_profile(
            profile,
            app_database_url,
            OUTBOX_MIGRATIONS,
            "Harvest workflow outbox",
        )?;
    }

    let harvest_database_url = match harvest_config.mode {
        HarvestMode::Embedded => app_config.database.url.as_deref().ok_or_else(|| {
            AutumnError::service_unavailable_msg(
                "autumn-harvest requires database.url when harvest.mode is embedded",
            )
        })?,
        HarvestMode::Split | HarvestMode::External => {
            harvest_config.database.url.as_deref().ok_or_else(|| {
                AutumnError::service_unavailable_msg(
                    "harvest.database.url is required for dedicated Harvest storage",
                )
            })?
        }
    };

    apply_migrations_for_profile(
        profile,
        harvest_database_url,
        HARVEST_MIGRATIONS,
        "Harvest storage",
    )
}

fn apply_migrations_for_profile(
    profile: &str,
    database_url: &str,
    migrations: EmbeddedMigrations,
    label: &str,
) -> autumn_web::AutumnResult<()> {
    if profile == "dev" {
        let result = autumn_web::migrate::run_pending(database_url, migrations)
            .map_err(|error| AutumnError::service_unavailable_msg(error.to_string()))?;
        if result.applied.is_empty() {
            tracing::info!(target = label, "No pending migrations");
        } else {
            for migration in result.applied {
                tracing::info!(target = label, migration = %migration, "Applied migration");
            }
        }
        return Ok(());
    }

    match autumn_web::migrate::pending_migrations(database_url, migrations) {
        Ok(pending) if pending.is_empty() => {
            tracing::info!(target = label, "Database migrations are up to date");
        }
        Ok(pending) => {
            tracing::warn!(
                target = label,
                count = pending.len(),
                "Pending migrations detected. Run `autumn migrate` to apply them."
            );
            for migration in pending {
                tracing::warn!(target = label, migration = %migration, "Pending migration");
            }
        }
        Err(error) => {
            tracing::warn!(target = label, error = %error, "Could not check migration status");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;

    use crate::config::{HarvestDatabaseConfig, HarvestMode, HarvestRuntimeConfig};
    use crate::runner::injected_runtime_state;
    use crate::{AppDbPool, HarvestDbPool};
    use autumn_harvest::dag::DagBuilder;
    use autumn_harvest::policy::Schedule;
    use autumn_web::config::DatabaseConfig;
    fn fake_workflow_info() -> WorkflowInfo {
        WorkflowInfo {
            name: "echo",
            module: "tests",
            handler: |_ctx, input| Box::pin(async move { Ok(input) }),
        }
    }

    fn fake_activity_info() -> ActivityInfo {
        ActivityInfo {
            name: "echo_activity",
            module: "tests",
            default_retry_policy: None,
            default_start_to_close: None,
            default_heartbeat_timeout: None,
            default_schedule_to_start: None,
            default_queue: None,
            handler: |_ctx, input| Box::pin(async move { Ok(input) }),
        }
    }

    fn fake_dag_info() -> DagInfo {
        fn build(_dag: &mut DagBuilder) {}

        DagInfo {
            name: "daily",
            module: "tests",
            schedule: Some(Schedule::Manual),
            catchup: false,
            max_active_runs: 1,
            default_queue: Some("default"),
            builder: build,
        }
    }

    fn test_app_state() -> AppState {
        AppState::for_test()
    }

    fn test_pool(database_url: &str, pool_size: usize) -> DbPool {
        autumn_web::create_pool(&DatabaseConfig {
            url: Some(database_url.to_owned()),
            pool_size,
            ..DatabaseConfig::default()
        })
        .expect("test pool config should build")
        .expect("test pool should exist")
    }

    #[test]
    fn harvest_ext_accumulates_registration_on_app_builder() {
        let builder = autumn_web::app()
            .workflows(vec![fake_workflow_info()])
            .activities(vec![fake_activity_info()])
            .dags(vec![fake_dag_info()])
            .state(String::from("haunted"))
            .worker(WorkerConfig::default().with_queues(["harvest"]))
            .harvest_api("/api/harvest");

        let integration = builder
            .extension::<HarvestIntegration>()
            .expect("harvest integration should be attached");
        assert!(integration.hooks_registered);

        let mut shared = integration.shared.lock().expect("harvest lock poisoned");
        assert_eq!(shared.registration.builder.workflow_count(), 1);
        assert_eq!(shared.registration.builder.activity_count(), 1);
        assert_eq!(shared.registration.builder.dag_count(), 1);
        assert_eq!(
            shared.registration.api_path.as_deref(),
            Some("/api/harvest")
        );

        let built = std::mem::take(&mut shared.registration.builder).build();
        drop(shared);
        assert_eq!(
            built.worker_config().queues.first().map(String::as_str),
            Some("harvest")
        );
        assert_eq!(built.state::<String>().map(String::as_str), Some("haunted"));
    }

    #[test]
    fn injected_runtime_state_contains_app_state() {
        let state = test_app_state();
        let harvest_pool = test_pool("postgres://harvest:harvest@localhost:5432/harvest", 4);
        let injected = injected_runtime_state(Some(state.clone()), None, harvest_pool);
        let stored = injected
            .get(&TypeId::of::<AppState>())
            .and_then(|value| value.downcast_ref::<AppState>())
            .expect("app state should be injected");

        assert_eq!(stored.profile(), state.profile());
    }

    #[test]
    fn harvest_ext_embedded_mode_reuses_app_pool() {
        let app_pool = test_pool("postgres://app:app@localhost:5432/app", 3);
        let state = AppState::for_test().with_pool(app_pool);
        let config = HarvestRuntimeConfig::default();

        let harvest_pool =
            resolve_harvest_pool(&state, &config).expect("embedded mode should reuse app pool");

        assert_eq!(harvest_pool.status().max_size, 3);
    }

    #[test]
    fn harvest_ext_split_mode_builds_dedicated_harvest_pool() {
        let app_pool = test_pool("postgres://app:app@localhost:5432/app", 3);
        let state = AppState::for_test().with_pool(app_pool.clone());
        let config = HarvestRuntimeConfig {
            mode: HarvestMode::Split,
            database: HarvestDatabaseConfig {
                url: Some("postgres://harvest:harvest@localhost:5432/harvest".to_owned()),
            },
            ..HarvestRuntimeConfig::default()
        };

        let harvest_pool = resolve_harvest_pool(&state, &config)
            .expect("split mode should resolve a dedicated harvest pool");

        assert_eq!(app_pool.status().max_size, 3);
        assert_eq!(harvest_pool.status().max_size, 10);
    }

    #[test]
    fn injected_runtime_state_contains_explicit_app_and_harvest_pool_roles() {
        let app_pool = test_pool("postgres://app:app@localhost:5432/app", 3);
        let harvest_pool = test_pool("postgres://harvest:harvest@localhost:5432/harvest", 7);
        let app_state = AppState::for_test().with_pool(app_pool.clone());
        let injected = injected_runtime_state(Some(app_state), Some(app_pool), harvest_pool);

        let app_db = injected
            .get(&TypeId::of::<AppDbPool>())
            .and_then(|value| value.downcast_ref::<AppDbPool>())
            .expect("app db pool should be injected");
        let harvest_db = injected
            .get(&TypeId::of::<HarvestDbPool>())
            .and_then(|value| value.downcast_ref::<HarvestDbPool>())
            .expect("harvest db pool should be injected");
        let legacy_harvest_db = injected
            .get(&TypeId::of::<DbPool>())
            .and_then(|value| value.downcast_ref::<DbPool>())
            .expect("legacy harvest db pool should still be injected");

        assert_eq!(app_db.status().max_size, 3);
        assert_eq!(harvest_db.status().max_size, 7);
        assert_eq!(legacy_harvest_db.status().max_size, 7);
    }

    #[test]
    fn harvest_ext_external_mode_builds_dedicated_harvest_pool() {
        let app_pool = test_pool("postgres://app:app@localhost:5432/app", 3);
        let state = AppState::for_test().with_pool(app_pool);
        let config = HarvestRuntimeConfig {
            mode: HarvestMode::External,
            worker_enabled: false,
            scheduler_enabled: false,
            database: HarvestDatabaseConfig {
                url: Some("postgres://harvest:harvest@localhost:5432/harvest".to_owned()),
            },
            outbox: crate::config::HarvestOutboxConfig::default(),
        };

        let harvest_pool = resolve_harvest_pool(&state, &config)
            .expect("external mode should resolve a dedicated harvest pool");

        assert_eq!(harvest_pool.status().max_size, 10);
    }
}
