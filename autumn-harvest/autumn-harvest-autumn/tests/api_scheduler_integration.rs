use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use autumn_harvest::builder::WorkerConfig;
use autumn_harvest::dag::DagBuilder;
use autumn_harvest::info::{ActivityInfo, DagInfo, WorkflowInfo};
use autumn_harvest::models::{DagRun, HarvestSchedule, WorkflowExecution};
use autumn_harvest::policy::Schedule;
use autumn_harvest::scheduler::{
    DagCatalog, SchedulerMonitor, compile_dag_catalog, register_schedules, tick_once,
};
use autumn_harvest::schema::{harvest_dag_runs, harvest_schedules, harvest_workflow_executions};
use autumn_harvest::worker::{DbPool, HandlerRegistry, Worker, WorkerRuntimeConfig};
use autumn_harvest::{ActivityContext, WorkflowContext};
use autumn_harvest_autumn::api::{HarvestApiRuntime, HarvestApiState, harvest_api_router};
use autumn_web::AppState;
use autumn_web::actuator::{ConfigProperties, LogLevels, TaskRegistry};
use autumn_web::middleware::MetricsCollector;
use autumn_web::reexports::axum;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use diesel::ExpressionMethods;
use diesel::OptionalExtension;
use diesel::QueryDsl;
use diesel::SelectableHelper;
use diesel_async::AsyncConnection;
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use serde_json::{Value, json};
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tower::ServiceExt;

const INIT_SQL: &str =
    include_str!("../../autumn-harvest/migrations/00000000000000_harvest_initial/up.sql");
type HarvestApiApp = axum::Router;

async fn setup_test_database_url() -> (String, ContainerAsync<Postgres>) {
    let container = Postgres::default()
        .with_init_sql(INIT_SQL.to_string().into_bytes())
        .start()
        .await
        .expect("failed to start Postgres container");

    let host = container
        .get_host()
        .await
        .expect("failed to get container host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to get container port");
    let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    (database_url, container)
}

fn build_test_pool(database_url: &str) -> DbPool {
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(database_url);
    deadpool::managed::Pool::builder(manager)
        .max_size(4)
        .build()
        .expect("failed to build test pool")
}

fn test_app_state(pool: DbPool) -> AppState {
    AppState {
        pool: Some(pool),
        profile: Some("test".to_string()),
        started_at: std::time::Instant::now(),
        health_detailed: true,
        metrics: MetricsCollector::new(),
        log_levels: LogLevels::new("info"),
        task_registry: TaskRegistry::new(),
        config_props: ConfigProperties::default(),
    }
}

fn build_test_worker(registry: Arc<HandlerRegistry>) -> Arc<Worker> {
    Arc::new(
        Worker::new(WorkerRuntimeConfig::from(WorkerConfig::default()), registry)
            .expect("worker config should be valid"),
    )
}

fn spawn_test_worker(worker: Arc<Worker>, pool: DbPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        worker.run(&pool).await;
    })
}

async fn shutdown_test_worker(worker: &Arc<Worker>, worker_task: tokio::task::JoinHandle<()>) {
    worker.shutdown();
    worker_task
        .await
        .expect("worker task should shut down cleanly");
}

async fn read_json_response(response: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&body).expect("response must be JSON")
}

async fn get_json(app: &HarvestApiApp, uri: impl Into<String>) -> (StatusCode, Value) {
    let uri = uri.into();
    let response = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .expect("GET request failed");
    let status = response.status();
    let json = read_json_response(response).await;
    (status, json)
}

async fn post_json(
    app: &HarvestApiApp,
    uri: impl Into<String>,
    payload: Value,
) -> (StatusCode, Value) {
    let uri = uri.into();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .expect("POST request failed");
    let status = response.status();
    let json = read_json_response(response).await;
    (status, json)
}

fn approval_registry() -> Arc<HandlerRegistry> {
    Arc::new(HandlerRegistry::new(
        vec![WorkflowInfo {
            name: "approval_workflow",
            module: "tests",
            handler: approval_workflow,
        }],
        vec![],
    ))
}

fn recording_activity_info(name: &'static str) -> ActivityInfo {
    ActivityInfo {
        name,
        module: "tests",
        default_retry_policy: None,
        default_start_to_close: None,
        default_heartbeat_timeout: None,
        default_schedule_to_start: None,
        default_queue: Some("default"),
        handler: record_activity,
    }
}

fn recording_registry(
    log: Arc<Mutex<Vec<String>>>,
    activity_names: &[&'static str],
) -> Arc<HandlerRegistry> {
    let mut state = HashMap::new();
    state.insert(
        std::any::TypeId::of::<Arc<Mutex<Vec<String>>>>(),
        Box::new(log) as Box<dyn std::any::Any + Send + Sync>,
    );

    Arc::new(HandlerRegistry::with_state(
        vec![],
        activity_names
            .iter()
            .copied()
            .map(recording_activity_info)
            .collect(),
        Arc::new(state),
    ))
}

async fn register_test_schedules(database_url: &str, dag_catalog: &DagCatalog, reason: &str) {
    let mut conn = <AsyncPgConnection as AsyncConnection>::establish(database_url)
        .await
        .expect(reason);
    register_schedules(&mut conn, dag_catalog)
        .await
        .expect("failed to register dag schedules");
}

async fn load_execution_from_url(database_url: &str, exec_id: &str) -> WorkflowExecution {
    let mut conn = <AsyncPgConnection as AsyncConnection>::establish(database_url)
        .await
        .expect("failed to connect fresh Postgres client for execution query");
    harvest_workflow_executions::table
        .find(
            exec_id
                .parse::<autumn_harvest::ExecutionId>()
                .expect("invalid execution id")
                .as_uuid(),
        )
        .select(WorkflowExecution::as_select())
        .first(&mut conn)
        .await
        .expect("failed to reload workflow execution")
}

async fn load_schedule_from_url(database_url: &str, dag_name: &str) -> HarvestSchedule {
    let mut conn = <AsyncPgConnection as AsyncConnection>::establish(database_url)
        .await
        .expect("failed to connect fresh Postgres client for schedule query");
    harvest_schedules::table
        .filter(harvest_schedules::dag_name.eq(dag_name))
        .select(HarvestSchedule::as_select())
        .first(&mut conn)
        .await
        .expect("failed to reload harvest schedule")
}

async fn load_latest_dag_run_from_url(database_url: &str, dag_name: &str) -> Option<DagRun> {
    let mut conn = <AsyncPgConnection as AsyncConnection>::establish(database_url)
        .await
        .expect("failed to connect fresh Postgres client for dag run query");
    harvest_dag_runs::table
        .filter(harvest_dag_runs::dag_name.eq(dag_name))
        .order(harvest_dag_runs::created_at.desc())
        .select(DagRun::as_select())
        .first(&mut conn)
        .await
        .optional()
        .expect("failed to reload latest dag run")
}

async fn wait_for_workflow_state(
    database_url: &str,
    exec_id: &str,
    expected_state: &str,
) -> WorkflowExecution {
    for _ in 0..100 {
        let execution = load_execution_from_url(database_url, exec_id).await;
        if execution.state == expected_state {
            return execution;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("workflow {exec_id} did not reach state {expected_state}");
}

async fn wait_for_dag_run_state(
    database_url: &str,
    dag_name: &str,
    expected_state: &str,
) -> DagRun {
    for _ in 0..100 {
        if let Some(run) = load_latest_dag_run_from_url(database_url, dag_name).await {
            if run.state == expected_state {
                return run;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("dag {dag_name} did not reach state {expected_state}");
}

fn approval_workflow<'a>(
    ctx: &'a WorkflowContext,
    input: Value,
) -> Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>> {
    Box::pin(async move {
        let request_id = input.get("request_id").cloned().unwrap_or(Value::Null);
        ctx.register_query("status", {
            let request_id = request_id.clone();
            move || {
                json!({
                    "phase": "waiting",
                    "request_id": request_id,
                })
            }
        });

        let approval = ctx
            .wait_for_signal("approved")
            .await
            .map_err(|error| error.to_string())?;

        Ok(json!({
            "phase": "approved",
            "approval": approval,
        }))
    })
}

fn record_activity<'a>(
    ctx: &'a ActivityContext,
    input: Value,
) -> Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>> {
    Box::pin(async move {
        let entries = ctx
            .state::<Arc<Mutex<Vec<String>>>>()
            .expect("shared log state must be registered");
        let step = input
            .get("dag_task")
            .or_else(|| input.get("step"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        entries
            .lock()
            .expect("log mutex poisoned")
            .push(step.clone());
        Ok(json!({ "step": step }))
    })
}

fn build_manual_pipeline_dag(dag: &mut DagBuilder) {
    const fn extract() {}
    const fn transform() {}
    const fn notify() {}

    let extract = dag.activity(extract);
    let transform = dag.activity(transform).upstream(&extract);
    let _notify = dag.activity(notify).upstream(&transform);
}

fn build_interval_pipeline_dag(dag: &mut DagBuilder) {
    const fn interval_step() {}

    let _step = dag.activity(interval_step);
}

fn manual_pipeline_info() -> DagInfo {
    DagInfo {
        name: "manual_pipeline",
        module: "tests",
        schedule: Some(Schedule::Manual),
        catchup: false,
        max_active_runs: 1,
        default_queue: Some("default"),
        builder: build_manual_pipeline_dag,
    }
}

fn interval_pipeline_info() -> DagInfo {
    DagInfo {
        name: "interval_pipeline",
        module: "tests",
        schedule: Some(Schedule::Interval(Duration::from_secs(1))),
        catchup: false,
        max_active_runs: 1,
        default_queue: Some("default"),
        builder: build_interval_pipeline_dag,
    }
}

#[tokio::test]
async fn harvest_api_starts_queries_and_signals_workflows() {
    let (database_url, _container) = setup_test_database_url().await;
    let pool = build_test_pool(&database_url);
    let registry = approval_registry();
    let api_state = HarvestApiState::new();
    api_state.install(HarvestApiRuntime::new(
        Arc::clone(&registry),
        Arc::new(HashMap::new()),
        "test-worker".to_string(),
        vec!["default".to_string()],
        SchedulerMonitor::offline(),
    ));

    let worker = build_test_worker(Arc::clone(&registry));
    let worker_task = spawn_test_worker(Arc::clone(&worker), pool.clone());

    let app = harvest_api_router(api_state.clone()).with_state(test_app_state(pool.clone()));

    let (start_status, start_json) = post_json(
        &app,
        "/workflows/approval_workflow/start",
        json!({
            "workflow_id": "approval-42",
            "input": { "request_id": "42" },
        }),
    )
    .await;
    assert_eq!(start_status, StatusCode::CREATED);
    let exec_id = start_json["execution_id"]
        .as_str()
        .expect("execution_id must be a string")
        .to_string();

    let (list_status, listed) = get_json(&app, "/workflows").await;
    assert_eq!(list_status, StatusCode::OK);
    assert!(
        listed
            .as_array()
            .expect("workflow list must be an array")
            .iter()
            .any(|row| row["id"] == exec_id),
        "started workflow should be listed"
    );

    let (query_status, query_json) =
        get_json(&app, format!("/workflows/{exec_id}/query/status")).await;
    assert_eq!(query_status, StatusCode::OK);
    assert_eq!(query_json["phase"], "waiting");
    assert_eq!(query_json["request_id"], "42");

    let (signal_status, _signal_json) = post_json(
        &app,
        format!("/workflows/{exec_id}/signal/approved"),
        json!({ "approved": true }),
    )
    .await;
    assert_eq!(signal_status, StatusCode::ACCEPTED);

    let execution = wait_for_workflow_state(&database_url, &exec_id, "COMPLETED").await;
    assert_eq!(execution.workflow_name, "approval_workflow");

    let (details_status, details_json) = get_json(&app, format!("/workflows/{exec_id}")).await;
    assert_eq!(details_status, StatusCode::OK);
    let history = details_json["history"]
        .as_array()
        .expect("workflow history must be an array");
    assert!(
        history
            .iter()
            .any(|event| event["type"] == "SignalReceived"),
        "history should include the delivered signal"
    );
    assert!(
        history
            .iter()
            .any(|event| event["type"] == "WorkflowCompleted"),
        "history should include workflow completion"
    );

    shutdown_test_worker(&worker, worker_task).await;
}

#[tokio::test]
async fn harvest_api_lists_and_triggers_manual_dags() {
    let (database_url, _container) = setup_test_database_url().await;
    let pool = build_test_pool(&database_url);
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let registry = recording_registry(Arc::clone(&log), &["extract", "transform", "notify"]);
    let dag_catalog = Arc::new(
        compile_dag_catalog(vec![manual_pipeline_info()])
            .expect("manual pipeline dag should compile"),
    );
    register_test_schedules(
        &database_url,
        dag_catalog.as_ref(),
        "failed to connect for schedule registration",
    )
    .await;

    let api_state = HarvestApiState::new();
    api_state.install(HarvestApiRuntime::new(
        Arc::clone(&registry),
        Arc::clone(&dag_catalog),
        "scheduler-only".to_string(),
        vec!["default".to_string()],
        SchedulerMonitor::offline(),
    ));
    let app = harvest_api_router(api_state).with_state(test_app_state(pool.clone()));

    let (dags_status, dags_json) = get_json(&app, "/dags").await;
    assert_eq!(dags_status, StatusCode::OK);
    assert!(
        dags_json
            .as_array()
            .expect("dags response must be an array")
            .iter()
            .any(|dag| dag["name"] == "manual_pipeline"),
        "registered dag should be listed"
    );

    let (trigger_status, _trigger_json) = post_json(
        &app,
        "/dags/manual_pipeline/trigger",
        json!({ "conf": { "step": "extract" } }),
    )
    .await;
    assert_eq!(trigger_status, StatusCode::CREATED);

    let run = wait_for_dag_run_state(&database_url, "manual_pipeline", "COMPLETED").await;
    assert_eq!(run.dag_name, "manual_pipeline");

    let (runs_status, runs_json) = get_json(&app, "/dags/manual_pipeline/runs").await;
    assert_eq!(runs_status, StatusCode::OK);
    assert!(
        runs_json
            .as_array()
            .expect("dag runs response must be an array")
            .iter()
            .any(|row| row["id"] == run.id.to_string()),
        "triggered dag run should be listed"
    );

    let recorded = log.lock().expect("log mutex poisoned").clone();
    assert_eq!(recorded, vec!["extract", "transform", "notify"]);
}

#[tokio::test]
async fn scheduler_tick_creates_and_executes_due_interval_runs() {
    let (database_url, _container) = setup_test_database_url().await;
    let pool = build_test_pool(&database_url);
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut state = HashMap::new();
    state.insert(
        std::any::TypeId::of::<Arc<Mutex<Vec<String>>>>(),
        Box::new(Arc::clone(&log)) as Box<dyn std::any::Any + Send + Sync>,
    );
    let registry = Arc::new(HandlerRegistry::with_state(
        vec![],
        vec![ActivityInfo {
            name: "interval_step",
            module: "tests",
            default_retry_policy: None,
            default_start_to_close: None,
            default_heartbeat_timeout: None,
            default_schedule_to_start: None,
            default_queue: Some("default"),
            handler: record_activity,
        }],
        Arc::new(state),
    ));
    let dag_catalog = Arc::new(
        compile_dag_catalog(vec![interval_pipeline_info()])
            .expect("interval pipeline dag should compile"),
    );

    {
        let mut conn = <AsyncPgConnection as AsyncConnection>::establish(&database_url)
            .await
            .expect("failed to connect for schedule registration");
        register_schedules(&mut conn, dag_catalog.as_ref())
            .await
            .expect("failed to register interval dag schedules");
    }

    let schedule = load_schedule_from_url(&database_url, "interval_pipeline").await;
    assert!(
        schedule.next_run_at.is_some(),
        "interval schedule should have next_run_at"
    );

    tick_once(
        pool.clone(),
        Arc::clone(&registry),
        Arc::clone(&dag_catalog),
        SchedulerMonitor::offline(),
    )
    .await
    .expect("scheduler tick should succeed");

    let run = wait_for_dag_run_state(&database_url, "interval_pipeline", "COMPLETED").await;
    assert_eq!(run.dag_name, "interval_pipeline");
    assert_eq!(
        log.lock().expect("log mutex poisoned").clone(),
        vec!["interval_step"]
    );
}
