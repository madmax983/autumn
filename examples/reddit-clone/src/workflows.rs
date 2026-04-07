//! Real autumn-harvest integration for the Reddit clone example.
//!
//! User registration enqueues durable onboarding that awards starter karma in
//! the background, and post submission enqueues durable publication work that
//! recalculates `hot_rank` and broadcasts the live-feed event. The app also
//! mounts Harvest's management API at `/api/harvest`.

use std::time::Duration;

use autumn_harvest::error::database_error;
use autumn_harvest::models::NewWorkflowExecution;
use autumn_harvest::prelude::*;
use autumn_harvest::queue::{EnqueueParams, TaskType};
use autumn_harvest::worker::DbPool;
use autumn_web::AppState;
use diesel::ExpressionMethods;
use diesel::QueryDsl;
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use serde_json::{Value, json};

use crate::models::User;
use crate::routes::live::publish_activity;
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
    let pool = ctx.state::<DbPool>().ok_or_else(|| {
        HarvestError::Config("reddit-clone Harvest activity is missing DbPool".into())
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
    let pool = ctx.state::<DbPool>().ok_or_else(|| {
        HarvestError::Config("reddit-clone Harvest activity is missing DbPool".into())
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
    let state = ctx.state::<AppState>().ok_or_else(|| {
        HarvestError::Config("reddit-clone Harvest activity is missing AppState".into())
    })?;
    let post_id = parse_i64_field(&input, "post_id", "post broadcast input")?;
    let title = parse_string_field(&input, "title", "post broadcast input")?;
    let post_slug = parse_string_field(&input, "post_slug", "post broadcast input")?;
    let subreddit_slug = parse_string_field(&input, "subreddit_slug", "post broadcast input")?;
    let author_username = parse_string_field(&input, "author_username", "post broadcast input")?;
    let event = json!({
        "type": "post_created",
        "post_id": post_id,
        "title": title,
        "post_slug": post_slug,
        "subreddit_slug": subreddit_slug,
        "author_username": author_username,
        "path": format!("/r/{subreddit_slug}/posts/{post_slug}"),
    });

    publish_activity(state, &subreddit_slug, &event.to_string());

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

pub async fn start_user_onboarding(
    conn: &mut AsyncPgConnection,
    user: &User,
) -> HarvestResult<ExecutionId> {
    let workflow_id = onboarding_workflow_id(user.id);
    let input = onboarding_input(user);

    start_workflow_execution(
        conn,
        ONBOARDING_WORKFLOW_NAME,
        &workflow_id,
        ONBOARDING_QUEUE,
        input,
        json!({
            "kind": "user_onboarding",
            "user_id": user.id,
        }),
        json!({
            "user_id": user.id,
            "username": user.username,
        }),
    )
    .await
}

pub async fn start_post_publication(
    conn: &mut AsyncPgConnection,
    post_id: i64,
    title: &str,
    post_slug: &str,
    subreddit_slug: &str,
    author_username: &str,
) -> HarvestResult<ExecutionId> {
    let workflow_id = post_publication_workflow_id(post_id);
    let input = post_publication_input(post_id, title, post_slug, subreddit_slug, author_username);

    start_workflow_execution(
        conn,
        POST_PUBLICATION_WORKFLOW_NAME,
        &workflow_id,
        POST_PUBLICATION_QUEUE,
        input,
        json!({
            "kind": "post_publication",
            "post_id": post_id,
        }),
        json!({
            "post_id": post_id,
            "post_slug": post_slug,
            "subreddit_slug": subreddit_slug,
            "author_username": author_username,
        }),
    )
    .await
}

fn onboarding_workflow_id(user_id: i64) -> String {
    format!("user-onboarding:{user_id}")
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

async fn start_workflow_execution(
    conn: &mut AsyncPgConnection,
    workflow_name: &'static str,
    workflow_id: &str,
    queue_name: &'static str,
    input: Value,
    memo: Value,
    search_attrs: Value,
) -> HarvestResult<ExecutionId> {
    let exec_id = ExecutionId::new();
    let row = NewWorkflowExecution {
        id: exec_id.as_uuid(),
        workflow_name,
        workflow_id,
        run_id: ExecutionId::new().as_uuid(),
        shard_id: 0,
        input: input.clone(),
        parent_id: None,
        queue_name,
        execution_timeout: None,
        memo: Some(memo),
        search_attrs: Some(search_attrs),
    };
    let started_event = WorkflowEvent::WorkflowStarted {
        input: input.clone(),
        timestamp: chrono::Utc::now(),
    };
    let mut params = EnqueueParams::new(queue_name, TaskType::Workflow, input);
    params.workflow_exec_id = Some(exec_id.as_uuid());

    diesel::insert_into(autumn_harvest::schema::harvest_workflow_executions::table)
        .values(&row)
        .execute(conn)
        .await
        .map_err(database_error)?;
    autumn_harvest::store::append_events(conn, exec_id, &[started_event], 0).await?;
    autumn_harvest::queue::enqueue(conn, &params).await?;

    Ok(exec_id)
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
}
