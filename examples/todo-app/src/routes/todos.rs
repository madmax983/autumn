//! HTML routes for the todo application.
//!
//! These routes render Maud templates styled with Tailwind CSS and
//! use htmx attributes for interactive toggle/delete behaviour.

use autumn::extract::{Form, Path};
use autumn::{AutumnError, AutumnResult, Db, Markup, delete, get, html, post};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{NewTodo, Todo};
use crate::schema::todos;

// ── Helpers ──────────────────────────────────────────────────────

/// Base HTML layout wrapping page content.
fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (autumn::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/css/autumn.css";
                script src="/static/js/htmx.min.js" {}
            }
            body class="bg-gray-100 min-h-screen" {
                div class="max-w-2xl mx-auto py-10 px-4" {
                    (content)
                }
            }
        }
    }
}

/// Render a single todo item as a list element with htmx controls.
fn todo_item(todo: &Todo) -> Markup {
    let check_classes = if todo.completed {
        "w-8 h-8 rounded-full border-2 border-green-500 bg-green-100 \
         text-green-600 font-bold flex items-center justify-center \
         cursor-pointer hover:bg-green-200 transition"
    } else {
        "w-8 h-8 rounded-full border-2 border-gray-300 \
         text-gray-400 flex items-center justify-center \
         cursor-pointer hover:border-green-400 hover:bg-green-50 transition"
    };

    let title_classes = if todo.completed {
        "line-through text-gray-400"
    } else {
        "text-gray-800"
    };

    html! {
        li id=(format!("todo-{}", todo.id))
           class="flex items-center gap-3 p-3 bg-white rounded shadow" {
            button hx-post=(format!("/todos/{}/toggle", todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML"
                   class=(check_classes) {
                @if todo.completed { "\u{2713}" } @else { "\u{25CB}" }
            }
            span class=(title_classes) {
                (todo.title)
            }
            button hx-delete=(format!("/todos/{}", todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML"
                   class="ml-auto text-red-500 hover:text-red-700 text-xl leading-none cursor-pointer" {
                "\u{00D7}"
            }
        }
    }
}

// ── Route handlers ───────────────────────────────────────────────

/// Redirect the root path to `/todos`.
#[get("/")]
pub async fn index() -> Markup {
    // Return a minimal page that redirects via meta refresh.
    // We avoid importing axum::response::Redirect to keep the example
    // focused on Autumn's own API surface.
    html! {
        (autumn::PreEscaped("<!DOCTYPE html>"))
        html {
            head {
                meta http-equiv="refresh" content="0;url=/todos";
            }
            body {
                p { "Redirecting to " a href="/todos" { "/todos" } "..." }
            }
        }
    }
}

/// List all todos.
#[get("/todos")]
pub async fn list(mut db: Db) -> AutumnResult<Markup> {
    let all_todos: Vec<Todo> = todos::table
        .order(todos::created_at.desc())
        .select(Todo::as_select())
        .load(&mut *db)
        .await?;

    Ok(layout(
        "Autumn Todo App",
        html! {
            header class="mb-8" {
                h1 class="text-3xl font-bold text-gray-800" { "Autumn Todos" }
                p class="text-gray-500 mt-1" { "A full-stack example with htmx + Tailwind" }
            }

            // New-todo form
            form action="/todos" method="post" class="flex gap-2 mb-6" {
                input type="text" name="title"
                      placeholder="What needs to be done?"
                      required
                      class="flex-1 px-4 py-2 border border-gray-300 rounded shadow-sm \
                             focus:outline-none focus:ring-2 focus:ring-blue-400";
                button type="submit"
                       class="px-6 py-2 bg-blue-600 text-white font-semibold rounded \
                              shadow hover:bg-blue-700 transition" {
                    "Add"
                }
            }

            // Todo list
            @if all_todos.is_empty() {
                p class="text-gray-400 text-center py-8" { "No todos yet. Add one above!" }
            } @else {
                ul id="todo-list" class="space-y-2" {
                    @for todo in &all_todos {
                        (todo_item(todo))
                    }
                }
            }
        },
    ))
}

/// Show a single todo by ID.
#[get("/todos/{id}")]
pub async fn detail(id: Path<i32>, mut db: Db) -> AutumnResult<Markup> {
    let todo: Todo = todos::table
        .find(*id)
        .select(Todo::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;

    Ok(layout(
        &format!("Todo: {}", todo.title),
        html! {
            a href="/todos" class="text-blue-600 hover:underline mb-4 inline-block" {
                "\u{2190} Back to list"
            }

            div class="bg-white rounded shadow p-6" {
                h2 class="text-2xl font-bold text-gray-800 mb-2" { (todo.title) }
                p class="text-gray-500" {
                    "Status: "
                    @if todo.completed {
                        span class="text-green-600 font-semibold" { "Completed" }
                    } @else {
                        span class="text-yellow-600 font-semibold" { "Pending" }
                    }
                }
                p class="text-gray-400 text-sm mt-2" {
                    "Created: " (todo.created_at.format("%Y-%m-%d %H:%M"))
                }
            }
        },
    ))
}

/// Create a new todo from a form submission, then redirect to the list.
#[post("/todos")]
pub async fn create(mut db: Db, form: Form<NewTodo>) -> AutumnResult<Markup> {
    let new_todo = form.0;

    if new_todo.title.trim().is_empty() {
        return Err(AutumnError::unprocessable(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Title must not be empty",
        )));
    }

    diesel::insert_into(todos::table)
        .values(&NewTodo {
            title: new_todo.title.trim().to_owned(),
        })
        .execute(&mut *db)
        .await?;

    // Redirect back to the todo list after creation
    Ok(html! {
        (autumn::PreEscaped("<!DOCTYPE html>"))
        html {
            head {
                meta http-equiv="refresh" content="0;url=/todos";
            }
            body {
                p { "Redirecting to " a href="/todos" { "/todos" } "..." }
            }
        }
    })
}

/// Toggle the completion status of a todo (htmx endpoint).
///
/// Returns the updated todo item HTML fragment for htmx to swap in.
#[post("/todos/{id}/toggle")]
pub async fn toggle(id: Path<i32>, mut db: Db) -> AutumnResult<Markup> {
    let todo: Todo = todos::table
        .find(*id)
        .select(Todo::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;

    diesel::update(todos::table.find(*id))
        .set(todos::completed.eq(!todo.completed))
        .execute(&mut *db)
        .await?;

    // Reload the updated todo for rendering
    let updated: Todo = todos::table
        .find(*id)
        .select(Todo::as_select())
        .first(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;

    Ok(todo_item(&updated))
}

/// Delete a todo by ID (htmx endpoint).
///
/// Returns an empty string so htmx removes the element from the DOM.
#[delete("/todos/{id}")]
pub async fn delete_todo(id: Path<i32>, mut db: Db) -> AutumnResult<String> {
    let deleted = diesel::delete(todos::table.find(*id))
        .execute(&mut *db)
        .await?;

    if deleted == 0 {
        return Err(AutumnError::not_found(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Todo with id {} not found", *id),
        )));
    }

    Ok(String::new())
}
