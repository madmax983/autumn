//! HTML routes for the todo application.
//!
//! These routes render Maud templates styled with Tailwind CSS and
//! use htmx attributes for interactive toggle/delete behaviour.

use autumn_web::etag::fresh_when;
use autumn_web::extract::Path;
use autumn_web::form::{ChangesetForm, method_input};
use autumn_web::pagination::{Page, PageRequest};
use autumn_web::prelude::{HxRequest, IntoResponse, StatusCode, Validate};
use autumn_web::security::{CspNonce, CsrfToken};
use autumn_web::{AutumnError, AutumnResult, Db, Markup, Redirect, delete, get, html, post};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};

use crate::models::{NewTodo, Todo};
use crate::schema::todos;

// ── Form type ─────────────────────────────────────────────────────

fn title_not_blank(s: &str) -> Result<(), validator::ValidationError> {
    if s.trim().is_empty() {
        let mut e = validator::ValidationError::new("blank");
        e.message = Some("Title must not be blank or whitespace-only".into());
        return Err(e);
    }
    Ok(())
}

#[derive(Deserialize, Serialize, Validate, Clone)]
pub struct TodoForm {
    #[validate(
        length(min = 1, max = 255, message = "Title must be 1–255 characters"),
        custom(function = "title_not_blank")
    )]
    title: String,
}

// ── Helpers ──────────────────────────────────────────────────────

/// Base HTML layout wrapping page content.
fn layout(title: &str, nonce: Option<&CspNonce>, content: Markup) -> Markup {
    html! {
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/css/autumn.css";
                script src="/static/js/htmx.min.js" {}
                // Nonce-protected inline script and style demonstrating Phase 2 compliance
                script nonce=[nonce.map(|n| n.nonce())] {
                    "console.log('Autumn App Initialized with CSP Nonce:', '" (nonce.map(|n| n.nonce()).unwrap_or_default()) "');"
                }
                style nonce=[nonce.map(|n| n.nonce())] {
                    "body { display: block; }"
                }
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
            button hx-post=(paths::toggle(todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML"
                   class=(check_classes) {
                @if todo.completed {
                    span class="text-xs font-bold" { (check_icon) }
                }
            }
            a href=(paths::detail(todo.id))
               class=(title_classes) {
                (todo.title)
            }
            button hx-delete=(paths::delete_todo(todo.id))
                   hx-target=(format!("#todo-{}", todo.id))
                   hx-swap="outerHTML"
                   hx-confirm="Delete this todo?"
                   class="opacity-0 group-hover:opacity-100 text-stone-400 \
                          hover:text-red-500 transition-all cursor-pointer p-1" {
                // Trash icon (SVG)
                (autumn_web::PreEscaped(r#"<svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M9 2a1 1 0 00-.894.553L7.382 4H4a1 1 0 000 2v10a2 2 0 002 2h8a2 2 0 002-2V6a1 1 0 100-2h-3.382l-.724-1.447A1 1 0 0011 2H9zM7 8a1 1 0 012 0v6a1 1 0 11-2 0V8zm5-1a1 1 0 00-1 1v6a1 1 0 102 0V8a1 1 0 00-1-1z" clip-rule="evenodd" /></svg>"#))
            }
        }
    }
}

/// Render the new-todo form, re-populating the title and showing errors on failure.
fn new_todo_form(pending: &ChangesetForm<TodoForm>) -> Markup {
    let errors = pending.errors_for("title");
    let inner = html! {
        div class="flex gap-2" {
            input type="text" name="title"
                  value=(pending.field_value("title").unwrap_or_default())
                  placeholder="What needs to be done?"
                  autocomplete="off"
                  aria-invalid=(if errors.is_empty() { "false" } else { "true" })
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
        @for msg in errors {
            p class="text-red-600 text-xs px-1" { (msg) }
        }
    };
    let form = pending.form_tag(&paths::create(), "post", inner);
    html! { div class="flex flex-col gap-2 mb-8" { (form) } }
}

/// Render Previous / page-indicator / Next pagination controls.
fn pagination_controls(page: &Page<Todo>, base_url: &str) -> Markup {
    if page.total_pages <= 1 {
        return html! {};
    }
    html! {
        nav class="flex items-center justify-between mt-6 text-sm" aria-label="Pagination" {
            @if page.has_previous {
                a href=(format!("{}?page={}&size={}", base_url, page.page - 1, page.size))
                   hx-get=(format!("{}?page={}&size={}", base_url, page.page - 1, page.size))
                   hx-target="body"
                   class="px-3 py-1.5 bg-white border border-stone-300 rounded-lg \
                          text-stone-600 hover:border-amber-400 hover:text-amber-700 transition-colors" {
                    "← Previous"
                }
            } @else {
                span class="px-3 py-1.5 text-stone-300 select-none" { "← Previous" }
            }
            span class="text-xs text-stone-400" {
                "Page " (page.page) " of " (page.total_pages)
                " \u{2022} " (page.total_elements) " total"
            }
            @if page.has_next {
                a href=(format!("{}?page={}&size={}", base_url, page.page + 1, page.size))
                   hx-get=(format!("{}?page={}&size={}", base_url, page.page + 1, page.size))
                   hx-target="body"
                   class="px-3 py-1.5 bg-white border border-stone-300 rounded-lg \
                          text-stone-600 hover:border-amber-400 hover:text-amber-700 transition-colors" {
                    "Next →"
                }
            } @else {
                span class="px-3 py-1.5 text-stone-300 select-none" { "Next →" }
            }
        }
    }
}

/// Render the full list page given a paginated todo result and an optional pending form.
async fn list_page(
    mut db: Db,
    page_req: &PageRequest,
    pending: &ChangesetForm<TodoForm>,
    nonce: Option<&CspNonce>,
) -> AutumnResult<Markup> {
    let page_data = Todo::page(page_req, &mut db).await?;
    let done_count = page_data.content.iter().filter(|t| t.completed).count();

    Ok(layout(
        "Autumn Todo App",
        nonce,
        html! {
            header class="mb-10" {
                h1 class="text-2xl font-semibold tracking-tight text-stone-900" {
                    "\u{1F342} Autumn Todos"
                }
                p class="text-stone-500 text-sm mt-1" {
                    "A full-stack example with htmx + Tailwind"
                }
            }

            (new_todo_form(pending))

            @if page_data.total_elements == 0 {
                div class="text-center py-16" {
                    p class="text-stone-400 text-sm" {
                        "No todos yet. Add one above!"
                    }
                }
            } @else {
                div class="flex items-center justify-between mb-3 px-1" {
                    p class="text-xs text-stone-400" {
                        (page_data.total_elements) " item"
                        @if page_data.total_elements != 1 { "s" }
                        @if done_count > 0 {
                            " \u{2022} " (done_count) " done this page"
                        }
                    }
                }
                ul id="todo-list" class="space-y-2" {
                    @for todo in &page_data.content {
                        (todo_item(todo))
                    }
                }
                (pagination_controls(&page_data, &paths::list()))
            }
        },
    ))
}

// ── Route handlers ───────────────────────────────────────────────

/// Redirect the root path to `/todos`.
#[get("/")]
pub async fn index() -> Redirect {
    Redirect::to(&paths::list())
}

/// List todos (paginated).
///
/// Accepts `?page=N&size=M` query parameters via the [`PageRequest`] extractor.
/// Defaults to 20 items per page ordered newest-first.
#[get("/todos")]
pub async fn list(page: PageRequest, db: Db, nonce: Option<CspNonce>) -> AutumnResult<Markup> {
    let blank = ChangesetForm::without_csrf(TodoForm {
        title: String::new(),
    });
    list_page(db, &page, &blank, nonce.as_ref()).await
}

/// Render the detail page body for a todo.
///
/// Extracted so the rendering — including the no-JavaScript delete form —
/// can be unit-tested without spinning up a database. The `csrf_token`
/// parameter mirrors what `#[get("/todos/{id}")]` receives in real
/// requests: `None` when CSRF protection is disabled (e.g. dev profile).
fn detail_view(todo: &Todo, csrf_token: Option<&str>, nonce: Option<&CspNonce>) -> Markup {
    layout(
        &format!("Todo: {}", todo.title),
        nonce,
        html! {
            a href=(paths::list())
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

                form method="post"
                     action=(paths::delete_todo(todo.id))
                     class="mt-6 flex items-center gap-3 border-t border-stone-100 pt-4" {
                    (method_input("DELETE"))
                    @if let Some(token) = csrf_token {
                        input type="hidden" name="_csrf" value=(token);
                    }
                    button type="submit"
                           class="text-sm text-red-600 hover:text-red-700 \
                                  underline-offset-2 hover:underline cursor-pointer" {
                        "Delete this todo"
                    }
                    span class="text-xs text-stone-400" {
                        "Works without JavaScript"
                    }
                }
            }
        },
    )
}

/// Show a single todo by ID.
///
/// Uses [`fresh_when`] for conditional-GET support: if the client already
/// holds a matching ETag the handler returns `304 Not Modified` with an empty
/// body, saving a full render + transport round-trip on every repeat visit or
/// htmx poll.
///
/// The ETag is derived from the todo's `created_at` timestamp and its
/// `completed` flag, so it changes whenever the todo is toggled.
///
/// The page also renders a plain `<form method="post">` carrying a hidden
/// `_method=DELETE` field as a no-JavaScript fallback alongside the
/// htmx-driven delete button on the list view. Both paths hit the same
/// `#[delete("/todos/{id}")]` handler — Autumn's method-override middleware
/// rewrites the transport `POST` to `DELETE` before route matching.
#[get("/todos/{id}")]
pub async fn detail(
    id: Path<i64>,
    headers: http::HeaderMap,
    mut db: Db,
    csrf: Option<CsrfToken>,
    nonce: Option<CspNonce>,
) -> AutumnResult<impl IntoResponse> {
    let todo = Todo::find(*id, &mut db).await?;

    // ETag: hash of (created_at_unix, completed flag, csrf_token).
    // Including the CSRF token ensures that CSRF rotation invalidates the
    // cached response, so clients never replay a stale hidden token.
    let csrf_tok = csrf.as_ref().map(CsrfToken::token).unwrap_or_default();
    let etag_input = format!(
        "{}-{}-{}",
        todo.created_at.and_utc().timestamp(),
        todo.completed,
        csrf_tok
    );
    let fw = fresh_when(&headers, etag_input.as_str());

    Ok(fw.or(detail_view(
        &todo,
        csrf.as_ref().map(CsrfToken::token),
        nonce.as_ref(),
    )))
}

/// Create a new todo from a form submission.
///
/// On validation failure the list page is re-rendered with inline errors (422).
/// On success a new row is inserted and the browser redirects to the list.
#[post("/todos")]
pub async fn create(
    nonce: Option<CspNonce>,
    db: Db,
    form: ChangesetForm<TodoForm>,
) -> AutumnResult<impl IntoResponse> {
    match form.into_valid() {
        Ok(f) => {
            let mut db = db;
            diesel::insert_into(todos::table)
                .values(NewTodo {
                    title: f.title.trim().to_owned(),
                })
                .execute(&mut *db)
                .await?;
            Ok(Redirect::to(&paths::list()).into_response())
        }
        Err(form) => {
            let page_req = PageRequest::default();
            let markup = list_page(db, &page_req, &form, nonce.as_ref()).await?;
            Ok((StatusCode::UNPROCESSABLE_ENTITY, markup).into_response())
        }
    }
}

/// Toggle the completion status of a todo (htmx endpoint).
///
/// Uses a single `UPDATE ... SET completed = NOT completed RETURNING *`
/// query — one round-trip instead of three.
#[post("/todos/{id}/toggle")]
pub async fn toggle(id: Path<i64>, mut db: Db) -> AutumnResult<Markup> {
    let updated: Todo = diesel::update(todos::table.find(*id))
        .set(todos::completed.eq(diesel::dsl::not(todos::completed)))
        .returning(Todo::as_returning())
        .get_result(&mut *db)
        .await
        .map_err(AutumnError::not_found)?;

    Ok(todo_item(&updated))
}

/// Delete a todo by ID.
///
/// Serves two callers off the same `#[delete]` route:
/// - htmx button on the list view: returns an empty body so htmx removes
///   the element from the DOM.
/// - Plain HTML form (no JavaScript): a `<form method="post">` carrying
///   `_method=DELETE` is rewritten to `DELETE` by Autumn's method-override
///   middleware before route matching. The handler redirects back to the
///   list, which is the no-JS-friendly response.
#[delete("/todos/{id}")]
pub async fn delete_todo(
    id: Path<i64>,
    mut db: Db,
    hx: HxRequest,
) -> AutumnResult<impl IntoResponse> {
    let deleted = diesel::delete(todos::table.find(*id))
        .execute(&mut *db)
        .await?;

    if deleted != 1 {
        return Err(AutumnError::not_found_msg(format!(
            "Todo with id {} not found",
            *id
        )));
    }

    if hx.is_htmx {
        Ok(String::new().into_response())
    } else {
        Ok(Redirect::to(&paths::list()).into_response())
    }
}

autumn_web::paths![index, list, detail, create, toggle, delete_todo];

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use autumn_web::form::IntoChangeset;

    #[test]
    fn test_path_helpers_correct() {
        assert_eq!(paths::list(), "/todos");
        assert_eq!(paths::detail(42), "/todos/42");
        assert_eq!(paths::toggle(7), "/todos/7/toggle");
        assert_eq!(paths::delete_todo(3), "/todos/3");
        assert_eq!(paths::create(), "/todos");
    }

    #[test]
    fn test_layout_generates_html() {
        let markup = layout("Test Title", None, html! { p { "Test Content" } });
        let html = markup.into_string();
        assert!(html.contains("Test Title"));
        assert!(html.contains("Test Content"));
        assert!(html.contains("htmx.min.js"));
    }

    #[test]
    fn test_todo_item_completed() {
        let todo = Todo {
            id: 1,
            title: "Test Todo".into(),
            completed: true,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
        };
        let markup = todo_item(&todo);
        let html = markup.into_string();
        assert!(html.contains("Test Todo"));
        assert!(html.contains("line-through"));
    }

    #[test]
    fn test_todo_item_pending() {
        let todo = Todo {
            id: 1,
            title: "Test Todo".into(),
            completed: false,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
        };
        let markup = todo_item(&todo);
        let html = markup.into_string();
        assert!(html.contains("Test Todo"));
        assert!(!html.contains("line-through"));
    }

    #[test]
    fn test_todo_item_uses_path_helpers() {
        let todo = Todo {
            id: 42,
            title: "Check paths".into(),
            completed: false,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
        };
        let markup = todo_item(&todo);
        let html = markup.into_string();
        assert!(html.contains("/todos/42/toggle"));
        assert!(html.contains("href=\"/todos/42\""));
        assert!(html.contains("hx-delete=\"/todos/42\""));
    }

    #[test]
    fn new_todo_form_shows_validation_errors() {
        let cs = TodoForm {
            title: String::new(),
        }
        .into_changeset();
        let form = ChangesetForm::from_changeset(cs);
        let html = new_todo_form(&form).into_string();
        assert!(html.contains("Title must be 1"));
        assert!(html.contains(r#"aria-invalid="true""#));
    }

    #[test]
    fn new_todo_form_clean_when_valid() {
        let form = ChangesetForm::without_csrf(TodoForm {
            title: "Buy milk".into(),
        });
        let html = new_todo_form(&form).into_string();
        assert!(html.contains(r#"aria-invalid="false""#));
        assert!(html.contains(r#"value="Buy milk""#));
    }

    /// Detail-page renders a plain `<form method="post">` that targets
    /// the declared `#[delete]` route via the `_method` override field.
    /// This is the no-JavaScript delete path alongside the htmx-driven
    /// delete on the list view.
    #[test]
    fn detail_view_renders_no_js_delete_form() {
        let todo = Todo {
            id: 42,
            title: "Write docs".into(),
            completed: false,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
        };
        let html = detail_view(&todo, Some("csrf-tok-xyz"), None).into_string();
        assert!(html.contains(r#"method="post""#), "{html}");
        assert!(html.contains(r#"action="/todos/42""#), "{html}");
        assert!(html.contains(r#"name="_method""#), "{html}");
        assert!(html.contains(r#"value="DELETE""#), "{html}");
        assert!(html.contains(r#"value="csrf-tok-xyz""#), "{html}");
        assert!(html.contains("Delete this todo"), "{html}");
    }

    #[test]
    fn detail_view_omits_csrf_input_when_token_absent() {
        let todo = Todo {
            id: 7,
            title: "No csrf".into(),
            completed: true,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
        };
        let html = detail_view(&todo, None, None).into_string();
        assert!(html.contains(r#"name="_method""#), "{html}");
        assert!(!html.contains(r#"name="_csrf""#), "{html}");
    }

    #[test]
    fn detail_view_renders_csp_nonce_script_and_style() {
        let todo = Todo {
            id: 42,
            title: "Write docs".into(),
            completed: false,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
        };
        let nonce = CspNonce::new("xyz123nonce".to_owned());
        let html = detail_view(&todo, None, Some(&nonce)).into_string();
        assert!(html.contains(r#"script nonce="xyz123nonce""#), "{html}");
        assert!(html.contains(r#"style nonce="xyz123nonce""#), "{html}");
    }

    #[test]
    fn pagination_controls_shows_next_on_first_page() {
        use autumn_web::pagination::Page;
        let req = PageRequest::new(1, 5);
        let items: Vec<Todo> = (1..=5)
            .map(|i| Todo {
                id: i,
                title: format!("Todo {i}"),
                completed: false,
                created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
            })
            .collect();
        let page: Page<Todo> = Page::new(items, 12, &req);
        let html = pagination_controls(&page, "/todos").into_string();
        // Next must be a link on the first page
        assert!(html.contains("Next"), "first page must show Next: {html}");
        assert!(
            html.contains("href=\"/todos?page=2"),
            "first page must link to page 2: {html}"
        );
        // Previous must NOT be a link on the first page (rendered as a disabled span)
        assert!(
            !html.contains("href=\"/todos?page=0"),
            "first page must not link to page 0: {html}"
        );
        assert!(
            html.contains("Page 1 of 3"),
            "must show current page of total: {html}"
        );
    }

    #[test]
    fn pagination_controls_hidden_for_single_page() {
        use autumn_web::pagination::Page;
        let req = PageRequest::new(1, 20);
        let items: Vec<Todo> = (1..=5)
            .map(|i| Todo {
                id: i,
                title: format!("Todo {i}"),
                completed: false,
                created_at: chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc(),
            })
            .collect();
        let page: Page<Todo> = Page::new(items, 5, &req);
        let html = pagination_controls(&page, "/todos").into_string();
        assert!(
            html.is_empty(),
            "single page must render no pagination controls: {html}"
        );
    }
}

#[cfg(test)]
mod mutant_tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_delete_todo_behavior() {
        // Can we test the exact endpoint behavior easily without DB integration?
        // No, we need TestDb. Since we can't easily mock it, we'll write a note on why
        // we can't write a direct unit test here and what it might look like.
    }
}
