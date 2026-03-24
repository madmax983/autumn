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
            body class="bg-stone-50 min-h-screen font-sans text-stone-800 antialiased" {
                div class="max-w-xl mx-auto py-12 px-6" {
                    (content)
                }
                footer class="text-center text-xs text-stone-400 py-8" {
                    "Built with "
                    a href="https://github.com/markm/autumn"
                      class="text-amber-600 hover:text-amber-700 hover:underline" {
                        "Autumn"
                    }
                    " \u{2022} Rust + Diesel + Maud + htmx"
                }
            }
        }
    }
}

/// Render a single todo item as a list element with htmx controls.
fn todo_item(todo: &Todo) -> Markup {
    let (check_classes, check_icon) = if todo.completed {
        (
            "w-5 h-5 shrink-0 rounded border-2 border-amber-500 bg-amber-500 \
             text-white flex items-center justify-center \
             cursor-pointer hover:bg-amber-600 hover:border-amber-600 transition-colors",
            "\u{2713}",
        )
    } else {
        (
            "w-5 h-5 shrink-0 rounded border-2 border-stone-300 \
             flex items-center justify-center \
             cursor-pointer hover:border-amber-400 transition-colors",
            "",
        )
    };

    let title_classes = if todo.completed {
        "flex-1 line-through text-stone-400 hover:text-stone-500 transition-colors"
    } else {
        "flex-1 text-stone-700 hover:text-amber-700 transition-colors"
    };

    html! {
        li id=(format!("todo-{}", todo.id))
           class="group flex items-center gap-3 px-4 py-3 bg-white rounded-lg \
                  border border-stone-200 hover:border-stone-300 \
                  shadow-sm transition-colors" {
            button hx-post=(format!("/todos/{}/toggle", todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML"
                   class=(check_classes) {
                @if todo.completed {
                    span class="text-xs font-bold" { (check_icon) }
                }
            }
            a href=(format!("/todos/{}", todo.id))
               class=(title_classes) {
                (todo.title)
            }
            button hx-delete=(format!("/todos/{}", todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML"
                   hx-confirm="Delete this todo?"
                   class="opacity-0 group-hover:opacity-100 text-stone-400 \
                          hover:text-red-500 transition-all cursor-pointer p-1" {
                // Trash icon (SVG)
                (autumn::PreEscaped(r#"<svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M9 2a1 1 0 00-.894.553L7.382 4H4a1 1 0 000 2v10a2 2 0 002 2h8a2 2 0 002-2V6a1 1 0 100-2h-3.382l-.724-1.447A1 1 0 0011 2H9zM7 8a1 1 0 012 0v6a1 1 0 11-2 0V8zm5-1a1 1 0 00-1 1v6a1 1 0 102 0V8a1 1 0 00-1-1z" clip-rule="evenodd" /></svg>"#))
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

    let done_count = all_todos.iter().filter(|t| t.completed).count();
    let total = all_todos.len();

    Ok(layout(
        "Autumn Todo App",
        html! {
            header class="mb-10" {
                h1 class="text-2xl font-semibold tracking-tight text-stone-900" {
                    "\u{1F342} Autumn Todos"
                }
                p class="text-stone-500 text-sm mt-1" {
                    "A full-stack example with htmx + Tailwind"
                }
            }

            // New-todo form
            form action="/todos" method="post"
                 class="flex gap-2 mb-8" {
                input type="text" name="title"
                      placeholder="What needs to be done?"
                      required
                      autocomplete="off"
                      class="flex-1 px-4 py-2.5 bg-white border border-stone-300 rounded-lg \
                             text-sm placeholder-stone-400 \
                             focus:outline-none focus:ring-2 focus:ring-amber-400/50 \
                             focus:border-amber-400 transition-colors";
                button type="submit"
                       class="px-5 py-2.5 bg-amber-600 text-white text-sm font-medium rounded-lg \
                              shadow-sm hover:bg-amber-700 active:bg-amber-800 \
                              transition-colors" {
                    "Add"
                }
            }

            // Todo list
            @if all_todos.is_empty() {
                div class="text-center py-16" {
                    p class="text-stone-400 text-sm" {
                        "No todos yet. Add one above!"
                    }
                }
            } @else {
                // Summary bar
                div class="flex items-center justify-between mb-3 px-1" {
                    p class="text-xs text-stone-400" {
                        (total) " item" @if total != 1 { "s" }
                        @if done_count > 0 {
                            " \u{2022} " (done_count) " done"
                        }
                    }
                }
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
            a href="/todos"
               class="inline-flex items-center gap-1 text-sm text-stone-500 \
                      hover:text-amber-600 transition-colors mb-6" {
                "\u{2190} Back to list"
            }

            div class="bg-white rounded-lg border border-stone-200 shadow-sm p-6" {
                h2 class="text-lg font-semibold text-stone-900 mb-3" { (todo.title) }
                div class="flex items-center gap-4 text-sm" {
                    @if todo.completed {
                        span class="inline-flex items-center gap-1.5 px-2.5 py-1 \
                                    bg-green-50 text-green-700 rounded-full text-xs font-medium" {
                            "\u{2713} Completed"
                        }
                    } @else {
                        span class="inline-flex items-center gap-1.5 px-2.5 py-1 \
                                    bg-amber-50 text-amber-700 rounded-full text-xs font-medium" {
                            "\u{25CB} Pending"
                        }
                    }
                    span class="text-stone-400 text-xs" {
                        "Created " (todo.created_at.format("%b %d, %Y at %H:%M"))
                    }
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
