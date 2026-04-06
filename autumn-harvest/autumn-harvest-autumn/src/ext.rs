//! Autumn `AppBuilder` integration for Harvest worker lifecycle.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use autumn_web::AppState;
use autumn_web::app::AppBuilder;
use autumn_web::error::AutumnError;
use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use tokio::task::JoinHandle;

use crate::api::{HarvestApiRuntime, HarvestApiState, harvest_api_router};
use autumn_harvest::builder::{HarvestBuilder, WorkerConfig};
use autumn_harvest::context::SharedStateMap;
use autumn_harvest::info::{ActivityInfo, DagInfo, WorkflowInfo};
use autumn_harvest::scheduler::{SchedulerMonitor, SchedulerRuntime, compile_dag_catalog};
use autumn_harvest::worker::{DbPool, Worker, WorkerRuntimeConfig};

const HARVEST_MIGRATIONS: EmbeddedMigrations = embed_migrations!("../autumn-harvest/migrations");

struct HarvestRuntime {
    worker: Arc<Worker>,
    worker_handle: JoinHandle<()>,
    scheduler: Option<SchedulerRuntime>,
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
        .migrations(HARVEST_MIGRATIONS)
        .on_startup(move |state| {
            let shared = Arc::clone(&startup_shared);
            let api_state = startup_api_state.clone();
            async move { start_harvest_runtime(state, &shared, &api_state) }
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
    state: AppState,
    shared: &Arc<Mutex<HarvestIntegrationShared>>,
    api_state: &HarvestApiState,
) -> autumn_web::AutumnResult<()> {
    let pool = state.pool().ok_or_else(|| {
        AutumnError::service_unavailable_msg("autumn-harvest requires a configured database")
    })?;

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
    let (registry, dags, worker_config) =
        built.into_worker_parts_with_extra_state(injected_runtime_state(state, Some(pool.clone())));
    let dag_catalog = Arc::new(
        compile_dag_catalog(dags)
            .map_err(|error| AutumnError::service_unavailable_msg(error.to_string()))?,
    );
    let runtime_config = WorkerRuntimeConfig::from(worker_config);
    let worker_id = runtime_config.worker_id.clone();
    let queues = runtime_config.queues.clone();
    let registry = Arc::new(registry);
    let worker = Arc::new(
        Worker::new(runtime_config, Arc::clone(&registry))
            .map_err(|error| AutumnError::service_unavailable_msg(error.to_string()))?,
    );
    let worker_pool = pool.clone();
    let worker_handle = {
        let worker = Arc::clone(&worker);
        tokio::spawn(async move {
            worker.run(&worker_pool).await;
        })
    };
    let scheduler = (!dag_catalog.is_empty()).then(|| {
        SchedulerRuntime::spawn(
            pool.clone(),
            Arc::clone(&registry),
            Arc::clone(&dag_catalog),
        )
    });
    let scheduler_monitor = scheduler
        .as_ref()
        .map_or_else(SchedulerMonitor::offline, SchedulerRuntime::monitor);
    api_state.install(HarvestApiRuntime::new(
        registry,
        dag_catalog,
        worker_id,
        queues,
        scheduler_monitor,
    ));

    {
        let mut guard = shared.lock().expect("harvest lock poisoned");
        guard.runtime = Some(HarvestRuntime {
            worker,
            worker_handle,
            scheduler,
        });
    }
    Ok(())
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

    runtime.worker.shutdown();
    if let Some(scheduler) = runtime.scheduler {
        scheduler.shutdown();
        if let Err(error) = scheduler.join().await {
            tracing::warn!(error = %error, "harvest scheduler task failed during shutdown");
        }
    }
    if let Err(error) = runtime.worker_handle.await {
        tracing::warn!(error = %error, "harvest worker task failed during shutdown");
    }
    api_state.clear();
}

fn injected_runtime_state(pool_state: AppState, pool: Option<DbPool>) -> SharedStateMap {
    let mut state: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
    state.insert(TypeId::of::<AppState>(), Box::new(pool_state));
    if let Some(pool) = pool {
        state.insert(TypeId::of::<DbPool>(), Box::new(pool));
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_harvest::dag::DagBuilder;
    use autumn_harvest::policy::Schedule;
    use autumn_web::actuator;
    use autumn_web::middleware;

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
        AppState {
            pool: None,
            profile: None,
            started_at: std::time::Instant::now(),
            health_detailed: true,
            metrics: middleware::MetricsCollector::new(),
            log_levels: actuator::LogLevels::new("info"),
            task_registry: actuator::TaskRegistry::new(),
            config_props: actuator::ConfigProperties::default(),
        }
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
        let injected = injected_runtime_state(state.clone(), None);
        let stored = injected
            .get(&TypeId::of::<AppState>())
            .and_then(|value| value.downcast_ref::<AppState>())
            .expect("app state should be injected");

        assert_eq!(stored.profile(), state.profile());
    }
}
