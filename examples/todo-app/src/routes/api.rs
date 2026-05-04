//! Token-secured JSON API routes for the todo application.
//!
//! Demonstrates how a mobile or CLI client would interact with the todo app:
//!
//! 1. Call `POST /api/tokens` to receive a bearer token.
//! 2. Include `Authorization: Bearer <token>` on every subsequent request.
//!
//! The HTML routes at `/todos` are unaffected — they use session auth as usual.

use autumn_web::auth::{DbApiTokenStore, issue_api_token};
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::Deserialize;

use crate::models::{NewTodo, Todo};
use crate::schema::todos;

// ── Token issuance ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct IssueTokenRequest {
    /// Arbitrary principal identifier, e.g. `"user:42"` or `"mobile-client"`.
    pub principal_id: String,
}

/// Issue a new Bearer token for the given principal.
///
/// Returns the raw token as plain text. **It is shown only once — store it
/// securely.** In production, protect this endpoint with session auth or an
/// admin guard; it is left open here for demonstration clarity.
#[post("/api/tokens")]
pub async fn issue_token(
    State(state): State<AppState>,
    body: Json<IssueTokenRequest>,
) -> AutumnResult<String> {
    let pool = state
        .pool()
        .expect("database required for API token issuance")
        .clone();
    let store = DbApiTokenStore::new(pool);
    issue_api_token(&store, &body.principal_id).await
}

// ── Protected todo endpoints ──────────────────────────────────────────────────
//
// These handlers are mounted behind `RequireApiToken` in main.rs.
// Requests without a valid `Authorization: Bearer <token>` header never reach
// them — the middleware rejects them with `401 Unauthorized` first.

/// Return all todos as a JSON array.
#[get("/api/todos")]
pub async fn list_json(ApiToken(caller): ApiToken, mut db: Db) -> AutumnResult<Json<Vec<Todo>>> {
    let _ = caller; // available for per-user filtering; unused in this demo
    let all_todos = Todo::all(&mut db).await?;
    Ok(Json(all_todos))
}

/// Create a new todo from a JSON body, return the created todo as JSON.
#[post("/api/todos")]
pub async fn create_json(
    ApiToken(caller): ApiToken,
    mut db: Db,
    body: Json<NewTodo>,
) -> AutumnResult<Json<Todo>> {
    let _ = caller;
    let new_todo = body.0.validated()?;
    let created: Todo = diesel::insert_into(todos::table)
        .values(&new_todo)
        .returning(Todo::as_returning())
        .get_result(&mut db)
        .await?;
    Ok(Json(created))
}
