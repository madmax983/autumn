use diesel::prelude::*;
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

/// Data needed to insert a new todo.
#[derive(Insertable, Deserialize)]
#[diesel(table_name = todos)]
pub struct NewTodo {
    pub title: String,
}
