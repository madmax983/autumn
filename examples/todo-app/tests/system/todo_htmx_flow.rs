//! System test: todo-app htmx swap flow.
//!
//! Covers the representative htmx user journey described in issue #816:
//!   1. Visit the todos list page.
//!   2. Fill in a new todo title.
//!   3. Submit the form (POST).
//!   4. Assert htmx settles (DOM swap completes).
//!   5. Assert the new item appears in the DOM.
//!
//! Run:
//!   cargo test -p todo-app --features system-tests -- --include-ignored
//!
//! Requires Chromium:
//!   apt-get install chromium-browser          # Ubuntu/Debian
//!   brew install --cask chromium              # macOS
//!   AUTUMN_CHROMIUM=/path/to/chrome cargo test # custom binary

#![cfg(feature = "system-tests")]

use std::sync::Mutex;

use autumn_web::prelude::*;
use autumn_web::reexports::axum;
use autumn_web::system_test::SystemTest;

// ── Minimal in-memory todo store ──────────────────────────────────────────
//
// The full todo-app requires Postgres; this test uses a process-global
// in-memory list to keep it self-contained and runnable without Docker.

static TODOS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn reset_store() {
    TODOS.lock().unwrap().clear();
}

// ── Route handlers ────────────────────────────────────────────────────────

#[get("/")]
async fn index() -> Markup {
    let todos = TODOS.lock().unwrap().clone();
    maud::html! {
        html {
            head {
                title { "Todos" }
                // htmx so settle detection exercises the real htmx path.
                script src="https://unpkg.com/htmx.org@1.9.10" {}
            }
            body {
                h1 { "My Todos" }
                ul id="todo-list" {
                    @for todo in &todos {
                        li { (todo) }
                    }
                }
                form
                    hx-post="/todos"
                    hx-target="#todo-list"
                    hx-swap="innerHTML"
                    method="post"
                    action="/todos" {
                    input
                        type="text"
                        name="title"
                        id="title"
                        placeholder="What needs doing?" {}
                    button type="submit" { "Add todo" }
                }
            }
        }
    }
}

#[post("/todos")]
async fn create_todo(
    axum_form: axum::extract::Form<std::collections::HashMap<String, String>>,
) -> Markup {
    let title = axum_form.0.get("title").cloned().unwrap_or_default();
    if !title.is_empty() {
        TODOS.lock().unwrap().push(title);
    }
    let todos = TODOS.lock().unwrap().clone();
    maud::html! {
        @for todo in &todos {
            li { (todo) }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

/// Happy-path: form submit → htmx swap → DOM assertion.
///
/// This is the representative system test described in issue #816:
///   form submit → 200 → htmx swap → assertion on new DOM content.
#[tokio::test]
#[ignore = "requires Chromium — set AUTUMN_CHROMIUM or install chromium-browser"]
async fn add_todo_htmx_swap() {
    reset_store();

    let mut runner = SystemTest::new()
        .routes(routes![index, create_todo])
        .build()
        .await
        .expect("system test runner — is Chromium installed?");

    let page = runner.page().await.expect("open browser page");

    // 1. Visit the todo list.
    page.visit("/").await.expect("visit /");
    page.expect_text("My Todos")
        .await
        .expect("page heading visible");

    // 2. Fill the title field.
    page.fill("input[name=title]", "Buy oat milk")
        .await
        .expect("fill title");

    // 3. Click submit — triggers the htmx POST.
    page.click("button[type=submit]")
        .await
        .expect("submit form");

    // 4. Wait for htmx to settle (auto-waited by click(); explicit for clarity).
    page.expect_hx_settle().await.expect("htmx settle");

    // 5. Assert the new item appears in the swapped DOM.
    page.expect_text("Buy oat milk")
        .await
        .expect("new todo visible after htmx swap");
}

/// Empty title submit does not add an empty item.
#[tokio::test]
#[ignore = "requires Chromium — set AUTUMN_CHROMIUM or install chromium-browser"]
async fn empty_title_not_added() {
    reset_store();

    let mut runner = SystemTest::new()
        .routes(routes![index, create_todo])
        .build()
        .await
        .expect("system test runner");

    let page = runner.page().await.expect("page");
    page.visit("/").await.expect("visit");
    // Submit without a title value.
    page.click("button[type=submit]").await.expect("submit");
    page.expect_hx_settle().await.expect("settle");

    // No items should appear.
    let has_li: bool = page
        .evaluate("document.querySelector('li') !== null")
        .await
        .expect("evaluate")
        .into_value()
        .unwrap_or(false);
    assert!(!has_li, "empty title must not add a list item");
}
