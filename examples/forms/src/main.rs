//! Changeset form example — create + edit + validation-failure round-trip.
//!
//! Demonstrates `autumn_web::form` in under 40 lines of route + template code:
//!
//! | Route | Handler |
//! |-------|---------|
//! | `GET  /users/new` | Renders a blank create-user form |
//! | `POST /users`     | Validates; on error re-renders 422 with inline messages |
//! | `GET  /users/{id}/edit` | Renders a pre-filled edit form |
//! | `PUT  /users/{id}` | Same validate-or-render logic as create |
//!
//! Run with:
//!
//! ```sh
//! cargo run -p forms
//! ```
//!
//! Then open <http://localhost:3000/users/new>.

use autumn_web::form::{ChangesetForm, form_tag, submit_button, text_input};
use autumn_web::prelude::*;
use serde::{Deserialize, Serialize};
use validator::Validate;

// ── Domain type ──────────────────────────────────────────────────

/// Three fields, three validators — the benchmark from the issue spec.
#[derive(Deserialize, Serialize, Validate, Clone)]
struct UserForm {
    #[validate(length(min = 3, max = 50, message = "Name must be 3–50 characters"))]
    name: String,
    #[validate(email(message = "Must be a valid email address"))]
    email: String,
    #[validate(length(min = 8, message = "Password must be at least 8 characters"))]
    password: String,
}

// ── Template helpers ─────────────────────────────────────────────

fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/css/autumn.css";
                // htmx is served automatically by autumn-web
                script src="/static/js/htmx.min.js" {}
            }
            body class="bg-stone-50 min-h-screen font-sans text-stone-800 antialiased" {
                div class="max-w-lg mx-auto py-12 px-6" {
                    h1 class="text-2xl font-semibold mb-8 text-stone-900" { (title) }
                    (content)
                }
            }
        }
    }
}

/// Shared form partial — same template for new and edit.
///
/// Passing `hx_post` wires up htmx so the validation round-trip is
/// a partial re-render rather than a full page reload.  Remove it and
/// the form falls back to a standard POST.
fn user_form_partial(cs: &Changeset<UserForm>, action: &str, method: &str) -> Markup {
    // NOTE: In a real app pass `Some(csrf.token())` as the third arg.
    // This example omits CSRF to keep the demo self-contained.
    form_tag(
        action,
        method,
        None,
        html! {
            div class="space-y-4" {
                (text_input(cs, "name", "Full name"))
                (text_input(cs, "email", "Email address"))
                (text_input(cs, "password", "Password"))
                (submit_button("Save"))
            }
        },
    )
}

// ── Create routes ────────────────────────────────────────────────

#[get("/users/new")]
async fn new_user() -> Markup {
    let blank = Changeset::new(UserForm {
        name: String::new(),
        email: String::new(),
        password: String::new(),
    });
    layout("New user", user_form_partial(&blank, "/users", "post"))
}

/// htmx round-trip: validation failure returns 422 so htmx re-renders
/// only the form partial without a full page reload.
/// Non-htmx fallback: browsers display the 422 page with inline errors.
#[post("/users")]
async fn create_user(ChangesetForm(cs): ChangesetForm<UserForm>) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;
    match cs.into_valid() {
        Ok(user) => {
            // In a real app: persist user, redirect to /users/{id}
            (
                StatusCode::OK,
                layout("User created", html! {
                    p class="text-green-700" {
                        "Created user: " (user.name) " <" (user.email) ">"
                    }
                    a href="/users/new" class="text-amber-600 underline" { "Create another" }
                }),
            )
        }
        Err(cs) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            layout("New user — fix errors", user_form_partial(&cs, "/users", "post")),
        ),
    }
}

// ── Edit routes ──────────────────────────────────────────────────

/// GET /users/{id}/edit — in a real app you'd load the record from DB.
#[get("/users/{id}/edit")]
async fn edit_user(id: Path<u64>) -> Markup {
    let prefilled = Changeset::new(UserForm {
        name: format!("User {}", *id),
        email: format!("user{}@example.com", *id),
        password: String::new(),
    });
    layout(
        "Edit user",
        user_form_partial(
            &prefilled,
            &format!("/users/{}", *id),
            "post", // browsers don't support PUT in forms; use POST + _method override
        ),
    )
}

#[post("/users/{id}")]
async fn update_user(
    id: Path<u64>,
    ChangesetForm(cs): ChangesetForm<UserForm>,
) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;
    match cs.into_valid() {
        Ok(user) => (
            StatusCode::OK,
            layout("User updated", html! {
                p class="text-green-700" { "Updated user " (*id) ": " (user.name) }
                a href=(format!("/users/{}/edit", *id)) class="text-amber-600 underline" { "Back" }
            }),
        ),
        Err(cs) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            layout(
                "Edit user — fix errors",
                user_form_partial(&cs, &format!("/users/{}", *id), "post"),
            ),
        ),
    }
}

// ── Main ─────────────────────────────────────────────────────────

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![new_user, create_user, edit_user, update_user])
        .run()
        .await;
}
