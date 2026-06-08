//! JSON API routes for the blog application.
//!
//! These endpoints provide a REST-style API alongside the HTML routes,
//! demonstrating that Autumn handlers can return either HTML or JSON.

use autumn_web::config::AutumnConfig;
use autumn_web::{AppState, AutumnResult, Db, Json, Path, get, job, post};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};

use crate::models::{NewPost, Post};
use crate::schema::posts;

/// Return all published posts as a JSON array.
#[get("/api/posts")]
pub async fn list_json(mut db: Db) -> AutumnResult<Json<Vec<Post>>> {
    let published = Post::published(&mut db).await?;
    Ok(Json(published))
}

/// Create a new post from a JSON body, return the created post as JSON.
#[post("/api/posts")]
pub async fn create_json(mut db: Db, body: Json<NewPost>) -> AutumnResult<Json<Post>> {
    let new_post = body.0.validated()?;

    let created: Post = diesel::insert_into(posts::table)
        .values(&new_post)
        .returning(Post::as_returning())
        .get_result(&mut db)
        .await?;

    Ok(Json(created))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishWebhookArgs {
    pub post_id: i64,
}

/// Demonstration ad-hoc background job.
#[job(name = "publish_webhook")]
pub async fn publish_webhook(_state: AppState, args: PublishWebhookArgs) -> AutumnResult<()> {
    eprintln!("publish_webhook job fired for post_id={}", args.post_id);
    Ok(())
}

/// Enqueue a background webhook publish job and return immediately.
#[post("/api/posts/{id}/enqueue-publish-webhook")]
pub async fn enqueue_publish_webhook(id: Path<i64>) -> AutumnResult<Json<serde_json::Value>> {
    PublishWebhookJob::enqueue(PublishWebhookArgs { post_id: *id }).await?;
    Ok(Json(serde_json::json!({
        "status": "queued",
        "job": "publish_webhook",
        "post_id": *id
    })))
}

/// Report whether a Stripe key is configured via the encrypted credentials store.
///
/// This endpoint demonstrates reading from `config/credentials/development.toml.enc`
/// at runtime via `config.credentials().get::<String>("stripe_secret_key")`.
/// Set `AUTUMN_MASTER_KEY` (or place the key in `config/master.key`) before running.
///
/// The actual key value is never included in the response — this is intentional.
#[get("/api/credentials-status")]
pub async fn credentials_status(config: AutumnConfig) -> Json<serde_json::Value> {
    let stripe_configured = config
        .credentials()
        .get::<String>("stripe_secret_key")
        .map(|k| !k.is_empty() && k != "sk_test_placeholder")
        .unwrap_or(false);

    let sendgrid_configured = config
        .credentials()
        .get::<String>("sendgrid_api_key")
        .map(|k| !k.is_empty() && k != "SG_placeholder")
        .unwrap_or(false);

    Json(serde_json::json!({
        "credentials_loaded": !config.credentials().is_empty(),
        "stripe_configured": stripe_configured,
        "sendgrid_configured": sendgrid_configured,
    }))
}
