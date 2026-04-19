//! Real autumn-harvest integration for the Reddit clone example.
//!
//! User registration enqueues durable onboarding that awards starter karma in
//! the background, and post submission enqueues durable publication work that
//! recalculates `hot_rank` and broadcasts the live-feed event. The app also
//! mounts Harvest's management API at `/api/harvest`.

use std::time::Duration;

use autumn_harvest::error::database_error;
use autumn_harvest::prelude::*;
use autumn_harvest_plugin::{AppDbPool, WorkflowStartRequest};
use autumn_web::AppState;
use diesel::ExpressionMethods;
use diesel::QueryDsl;
use diesel_async::RunQueryDsl;
use serde_json::{Value, json};

#[cfg(test)]
use autumn_harvest::{StartWorkflowParams, start_or_load_workflow_execution};
#[cfg(test)]
use diesel_async::AsyncPgConnection;

use crate::live_events::{
    post_created_event, publish_stored_live_event_best_effort, store_activity_event,
};
use crate::models::User;
use crate::schema::posts;
use crate::schema::users;
use crate::tasks::calculate_hot_rank;

const ONBOARDING_WORKFLOW_NAME: &str = "user_onboarding";
const ONBOARDING_QUEUE: &str = "default";
const POST_PUBLICATION_WORKFLOW_NAME: &str = "post_publication";
const POST_PUBLICATION_QUEUE: &str = "default";
const STARTER_KARMA: i64 = 5;

#[workflow]
async fn user_onboarding(ctx: &WorkflowContext, input: Value) -> HarvestResult<Value> {
    let user_id = parse_user_id(&input)?;
    let username = parse_username(&input)?;
    let activity_output = ctx
        .execute_activity_raw(
            "award_starter_karma",
            json!({
                "user_id": user_id,
                "starter_karma": STARTER_KARMA,
            }),
            ONBOARDING_QUEUE,
        )
        .await?;

    Ok(json!({
        "status": "completed",
        "user_id": user_id,
        "username": username,
        "starter_karma": activity_output["starter_karma"],
    }))
}

#[workflow]
async fn post_publication(ctx: &WorkflowContext, input: Value) -> HarvestResult<Value> {
    let post_id = parse_i64_field(&input, "post_id", "post publication input")?;
    let title = parse_string_field(&input, "title", "post publication input")?;
    let post_slug = parse_string_field(&input, "post_slug", "post publication input")?;
    let subreddit_slug = parse_string_field(&input, "subreddit_slug", "post publication input")?;
    let author_username = parse_string_field(&input, "author_username", "post publication input")?;
    let hot_rank_output = ctx
        .execute_activity_raw(
            "refresh_post_hot_rank",
            json!({
                "post_id": post_id,
            }),
            POST_PUBLICATION_QUEUE,
        )
        .await?;
    let broadcast_output = ctx
        .execute_activity_raw(
            "broadcast_post_created",
            json!({
                "post_id": post_id,
                "title": title,
                "post_slug": post_slug,
                "subreddit_slug": subreddit_slug,
                "author_username": author_username,
            }),
            POST_PUBLICATION_QUEUE,
        )
        .await?;

    Ok(json!({
        "status": "completed",
        "post_id": post_id,
        "hot_rank": hot_rank_output["hot_rank"],
        "event": broadcast_output["event"],
    }))
}

#[activity(
    start_to_close = "10s",
    retry = RetryPolicy::fixed(3, Duration::from_secs(1))
)]
async fn award_starter_karma(ctx: &ActivityContext, input: Value) -> HarvestResult<Value> {
    let user_id = parse_user_id(&input)?;
    let starter_karma = input
        .get("starter_karma")
        .and_then(Value::as_i64)
        .unwrap_or(STARTER_KARMA);
    let pool = ctx.state::<AppDbPool>().ok_or_else(|| {
        HarvestError::Config("reddit-clone Harvest activity is missing AppDbPool".into())
    })?;
    let mut conn = pool.get().await.map_err(database_error)?;

    diesel::update(users::table.filter(users::id.eq(user_id)))
        .set(users::karma.eq(starter_karma))
        .execute(&mut conn)
        .await
        .map_err(database_error)?;

    Ok(json!({
        "user_id": user_id,
        "starter_karma": starter_karma,
    }))
}

#[activity(
    start_to_close = "10s",
    retry = RetryPolicy::fixed(3, Duration::from_secs(1))
)]
async fn refresh_post_hot_rank(ctx: &ActivityContext, input: Value) -> HarvestResult<Value> {
    let post_id = parse_i64_field(&input, "post_id", "post hot-rank input")?;
    let pool = ctx.state::<AppDbPool>().ok_or_else(|| {
        HarvestError::Config("reddit-clone Harvest activity is missing AppDbPool".into())
    })?;
    let mut conn = pool.get().await.map_err(database_error)?;
    let (score, created_at): (i64, chrono::NaiveDateTime) = posts::table
        .find(post_id)
        .select((posts::score, posts::created_at))
        .first(&mut conn)
        .await
        .map_err(database_error)?;
    let hot_rank = calculate_hot_rank(score, created_at, chrono::Utc::now().naive_utc());

    diesel::update(posts::table.find(post_id))
        .set(posts::hot_rank.eq(hot_rank))
        .execute(&mut conn)
        .await
        .map_err(database_error)?;

    Ok(json!({
        "post_id": post_id,
        "hot_rank": hot_rank,
    }))
}

#[activity(
    start_to_close = "10s",
    retry = RetryPolicy::fixed(3, Duration::from_secs(1))
)]
async fn broadcast_post_created(ctx: &ActivityContext, input: Value) -> HarvestResult<Value> {
    let pool = ctx.state::<AppDbPool>().ok_or_else(|| {
        HarvestError::Config("reddit-clone Harvest activity is missing AppDbPool".into())
    })?;
    let mut conn = pool.get().await.map_err(database_error)?;
    let post_id = parse_i64_field(&input, "post_id", "post broadcast input")?;
    let title = parse_string_field(&input, "title", "post broadcast input")?;
    let post_slug = parse_string_field(&input, "post_slug", "post broadcast input")?;
    let subreddit_slug = parse_string_field(&input, "subreddit_slug", "post broadcast input")?;
    let author_username = parse_string_field(&input, "author_username", "post broadcast input")?;
    let event = post_created_event(
        post_id,
        &title,
        &post_slug,
        &subreddit_slug,
        &author_username,
    );
    let event_id = store_activity_event(&mut conn, &subreddit_slug, &event)
        .await
        .map_err(database_error)?;
    if let Some(state) = ctx.state::<AppState>() {
        publish_stored_live_event_best_effort(state, event_id).await;
    } else {
        tracing::warn!(
            event_id,
            "reddit-clone Harvest activity is missing AppState for live-event bus publication"
        );
    }

    Ok(json!({
        "post_id": post_id,
        "event": event,
    }))
}

pub fn registered_workflows() -> Vec<WorkflowInfo> {
    workflows![user_onboarding, post_publication]
}

pub fn registered_activities() -> Vec<ActivityInfo> {
    activities![
        award_starter_karma,
        refresh_post_hot_rank,
        broadcast_post_created
    ]
}

fn onboarding_workflow_id(user_id: i64) -> String {
    format!("user-onboarding:{user_id}")
}

pub(crate) fn user_onboarding_dispatch(user: &User) -> WorkflowStartRequest {
    WorkflowStartRequest {
        workflow_name: ONBOARDING_WORKFLOW_NAME.to_string(),
        workflow_id: onboarding_workflow_id(user.id),
        queue_name: ONBOARDING_QUEUE.to_string(),
        input: onboarding_input(user),
        memo: Some(json!({
            "kind": "user_onboarding",
            "user_id": user.id,
        })),
        search_attrs: Some(json!({
            "user_id": user.id,
            "username": user.username,
        })),
    }
}

fn onboarding_input(user: &User) -> Value {
    json!({
        "user_id": user.id,
        "username": user.username,
    })
}

fn post_publication_workflow_id(post_id: i64) -> String {
    format!("post-publication:{post_id}")
}

pub(crate) fn post_publication_dispatch(
    post_id: i64,
    title: &str,
    post_slug: &str,
    subreddit_slug: &str,
    author_username: &str,
) -> WorkflowStartRequest {
    WorkflowStartRequest {
        workflow_name: POST_PUBLICATION_WORKFLOW_NAME.to_string(),
        workflow_id: post_publication_workflow_id(post_id),
        queue_name: POST_PUBLICATION_QUEUE.to_string(),
        input: post_publication_input(post_id, title, post_slug, subreddit_slug, author_username),
        memo: Some(json!({
            "kind": "post_publication",
            "post_id": post_id,
        })),
        search_attrs: Some(json!({
            "post_id": post_id,
            "post_slug": post_slug,
            "subreddit_slug": subreddit_slug,
            "author_username": author_username,
        })),
    }
}

fn post_publication_input(
    post_id: i64,
    title: &str,
    post_slug: &str,
    subreddit_slug: &str,
    author_username: &str,
) -> Value {
    json!({
        "post_id": post_id,
        "title": title,
        "post_slug": post_slug,
        "subreddit_slug": subreddit_slug,
        "author_username": author_username,
    })
}

#[cfg(test)]
async fn start_workflow_execution(
    conn: &mut AsyncPgConnection,
    workflow_name: &str,
    workflow_id: &str,
    queue_name: &str,
    input: Value,
    memo: Option<Value>,
    search_attrs: Option<Value>,
) -> HarvestResult<ExecutionId> {
    let start = start_or_load_workflow_execution(
        conn,
        StartWorkflowParams {
            workflow_name,
            workflow_id,
            shard_id: 0,
            input,
            parent_id: None,
            queue_name,
            execution_timeout: None,
            memo,
            search_attrs,
        },
    )
    .await?;

    Ok(start.exec_id)
}

fn parse_user_id(input: &Value) -> HarvestResult<i64> {
    parse_i64_field(input, "user_id", "user onboarding input")
}

fn parse_username(input: &Value) -> HarvestResult<String> {
    parse_string_field(input, "username", "user onboarding input")
}

fn parse_i64_field(input: &Value, field: &str, context: &str) -> HarvestResult<i64> {
    input
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| HarvestError::Config(format!("{context} is missing numeric {field}")))
}

fn parse_string_field(input: &Value, field: &str, context: &str) -> HarvestResult<String> {
    input
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| HarvestError::Config(format!("{context} is missing {field}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::ExpressionMethods;
    use diesel::QueryDsl;
    use diesel::SelectableHelper;
    use diesel_async::AsyncConnection;
    use diesel_async::AsyncPgConnection;
    use diesel_async::RunQueryDsl;
    use testcontainers::ContainerAsync;
    use testcontainers_modules::postgres::Postgres;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;

    const HARVEST_INIT_SQL: &str = include_str!(
        "../../../autumn-harvest/autumn-harvest/migrations/20260409000000_harvest_initial/up.sql"
    );

    #[tokio::test]
    #[ignore = "requires Docker (testcontainers)"]
    async fn duplicate_publication_reuses_existing_execution() {
        let (mut conn, _container) = setup_test_db().await;
        let workflow_id = onboarding_workflow_id(42);
        let input = json!({
            "user_id": 42,
            "username": "ferris",
        });

        let first_exec_id = start_workflow_execution(
            &mut conn,
            ONBOARDING_WORKFLOW_NAME,
            &workflow_id,
            ONBOARDING_QUEUE,
            input.clone(),
            Some(json!({
                "kind": "user_onboarding",
                "user_id": 42,
            })),
            Some(json!({
                "user_id": 42,
                "username": "ferris",
            })),
        )
        .await
        .expect("first publication should succeed");

        let second_exec_id = start_workflow_execution(
            &mut conn,
            ONBOARDING_WORKFLOW_NAME,
            &workflow_id,
            ONBOARDING_QUEUE,
            input,
            Some(json!({
                "kind": "user_onboarding",
                "user_id": 42,
            })),
            Some(json!({
                "user_id": 42,
                "username": "ferris",
            })),
        )
        .await
        .expect("duplicate publication should resolve safely");

        assert_eq!(
            second_exec_id, first_exec_id,
            "duplicate publication should return the original execution id"
        );

        let executions =
            load_workflow_rows(&mut conn, ONBOARDING_WORKFLOW_NAME, &workflow_id).await;
        assert_eq!(
            executions.len(),
            1,
            "duplicate publication should not create a second workflow row"
        );
        let queued_tasks = count_workflow_tasks(&mut conn, first_exec_id).await;
        assert_eq!(
            queued_tasks, 1,
            "duplicate publication should not enqueue duplicate tasks"
        );
    }

    #[test]
    fn onboarding_metadata_uses_expected_names() {
        let workflows = registered_workflows();
        let activities = registered_activities();

        assert_eq!(workflows.len(), 2);
        assert!(
            workflows
                .iter()
                .any(|workflow| workflow.name == ONBOARDING_WORKFLOW_NAME)
        );
        assert!(
            workflows
                .iter()
                .any(|workflow| workflow.name == POST_PUBLICATION_WORKFLOW_NAME)
        );
        assert_eq!(activities.len(), 3);
        assert!(
            activities
                .iter()
                .any(|activity| activity.name == "award_starter_karma")
        );
        assert!(
            activities
                .iter()
                .any(|activity| activity.name == "refresh_post_hot_rank")
        );
        assert!(
            activities
                .iter()
                .any(|activity| activity.name == "broadcast_post_created")
        );
    }

    #[test]
    fn onboarding_input_contains_user_identity() {
        let user = User {
            id: 42,
            username: "ferris".to_string(),
            password_hash: "hashed".to_string(),
            karma: 0,
            role: "user".to_string(),
            created_at: chrono::DateTime::UNIX_EPOCH.naive_utc(),
        };

        assert_eq!(onboarding_workflow_id(user.id), "user-onboarding:42");
        assert_eq!(
            onboarding_input(&user),
            json!({
                "user_id": 42,
                "username": "ferris",
            })
        );
    }

    #[test]
    fn post_publication_input_contains_post_identity() {
        assert_eq!(post_publication_workflow_id(99), "post-publication:99");
        assert_eq!(
            post_publication_input(99, "Ferris arrives", "ferris-arrives", "rust", "ferris"),
            json!({
                "post_id": 99,
                "title": "Ferris arrives",
                "post_slug": "ferris-arrives",
                "subreddit_slug": "rust",
                "author_username": "ferris",
            })
        );
    }

    async fn setup_test_db() -> (AsyncPgConnection, ContainerAsync<Postgres>) {
        let container = Postgres::default()
            .with_init_sql(HARVEST_INIT_SQL.to_string().into_bytes())
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

        let conn = <AsyncPgConnection as AsyncConnection>::establish(&database_url)
            .await
            .expect("failed to connect to Postgres container");

        (conn, container)
    }

    async fn load_workflow_rows(
        conn: &mut AsyncPgConnection,
        workflow_name: &str,
        workflow_id: &str,
    ) -> Vec<autumn_harvest::models::WorkflowExecution> {
        autumn_harvest::schema::harvest_workflow_executions::table
            .filter(
                autumn_harvest::schema::harvest_workflow_executions::workflow_name
                    .eq(workflow_name),
            )
            .filter(
                autumn_harvest::schema::harvest_workflow_executions::workflow_id.eq(workflow_id),
            )
            .order(autumn_harvest::schema::harvest_workflow_executions::created_at.asc())
            .select(autumn_harvest::models::WorkflowExecution::as_select())
            .load(conn)
            .await
            .expect("failed to load workflow rows")
    }

    async fn count_workflow_tasks(conn: &mut AsyncPgConnection, exec_id: ExecutionId) -> i64 {
        autumn_harvest::schema::harvest_task_queue::table
            .filter(
                autumn_harvest::schema::harvest_task_queue::workflow_exec_id
                    .eq(Some(exec_id.as_uuid())),
            )
            .count()
            .get_result(conn)
            .await
            .expect("failed to count workflow task rows")
    }
}
