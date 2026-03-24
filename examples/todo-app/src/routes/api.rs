//! JSON API routes for the todo application.
//!
//! These endpoints provide a REST-style API alongside the HTML routes,
//! demonstrating that Autumn handlers can return either HTML or JSON.

use autumn::{AutumnError, AutumnResult, Db, Json, get, post};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{NewTodo, Todo};
use crate::schema::todos;

/// Return all todos as a JSON array.
#[get("/api/todos")]
pub async fn list_json(mut db: Db) -> AutumnResult<Json<Vec<Todo>>> {
    let all_todos = todos::table
        .order(todos::created_at.desc())
        .select(Todo::as_select())
        .load(&mut *db)
        .await?;

    Ok(Json(all_todos))
}

/// Create a new todo from a JSON body, return the created todo as JSON.
#[post("/api/todos")]
pub async fn create_json(mut db: Db, body: Json<NewTodo>) -> AutumnResult<Json<Todo>> {
    let new_todo = body.0;

    if new_todo.title.trim().is_empty() {
        return Err(AutumnError::unprocessable(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Title must not be empty",
        )));
    }

    let created: Todo = diesel::insert_into(todos::table)
        .values(&NewTodo {
            title: new_todo.title.trim().to_owned(),
        })
        .returning(Todo::as_returning())
        .get_result(&mut *db)
        .await?;

    Ok(Json(created))
}
