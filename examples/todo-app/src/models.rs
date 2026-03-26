use autumn_web::error::{AutumnError, AutumnResult};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::{Deserialize, Serialize};

use crate::schema::todos;

/// A todo item loaded from the database.
#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = todos)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Todo {
    pub id: i32,
    pub title: String,
    pub completed: bool,
    pub created_at: chrono::NaiveDateTime,
}

impl Todo {
    /// Load all todos ordered by creation date (newest first).
    pub async fn all(db: &mut AsyncPgConnection) -> AutumnResult<Vec<Self>> {
        Ok(todos::table
            .order(todos::created_at.desc())
            .select(Self::as_select())
            .load(db)
            .await?)
    }

    /// Find a single todo by ID, returning 404 if not found.
    pub async fn find(id: i32, db: &mut AsyncPgConnection) -> AutumnResult<Self> {
        todos::table
            .find(id)
            .select(Self::as_select())
            .first(db)
            .await
            .map_err(AutumnError::not_found)
    }
}

/// Data needed to insert a new todo.
#[derive(Insertable, Deserialize)]
#[diesel(table_name = todos)]
pub struct NewTodo {
    pub title: String,
}

impl NewTodo {
    /// Validate and normalize the title. Returns 422 if the title is empty.
    pub fn validated(self) -> AutumnResult<Self> {
        let title = self.title.trim().to_owned();
        if title.is_empty() {
            return Err(AutumnError::unprocessable_msg("Title must not be empty"));
        }
        Ok(Self { title })
    }
}
