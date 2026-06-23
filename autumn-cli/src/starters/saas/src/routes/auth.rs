//! Signup, login, and logout.
//!
//! Composes shipped primitives: `Session` for the cookie-backed session,
//! `hash_password`/`verify_password` (bcrypt) for credentials, and plain Diesel
//! for the user row. On success we store both `user_id` and `tenant_id` in the
//! session; the dashboard reads `tenant_id` back to scope every query.

use autumn_web::auth::{hash_password, verify_password};
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::Deserialize;

use crate::models::{NewUser, User};
use crate::schema::users;

use super::layout::layout;

#[derive(Deserialize)]
pub struct SignupForm {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
}

// bcrypt hash used as a dummy target when the email is not found, so the
// login handler takes the same wall time whether or not the account exists.
const DUMMY_HASH: &str =
    "$2b$12$Ro0CUfOqk6cXEKf3dyaM7OhSCvnwM9s1Aw6lfLP2.GvpAfNXwi.2K";

// ── Signup ───────────────────────────────────────────────────────────────────

#[get("/signup")]
pub async fn signup_form() -> Markup {
    layout(
        "Sign up",
        false,
        html! {
            h1 class="text-2xl font-bold mb-6" { "Create your account" }
            form action="/signup" method="post" class="space-y-4 bg-white rounded-lg shadow p-6 max-w-md" {
                div {
                    label for="email" class="block text-sm font-medium mb-1" { "Email" }
                    input #email type="email" name="email" required autocomplete="email"
                          class="w-full border rounded px-3 py-2";
                }
                div {
                    label for="password" class="block text-sm font-medium mb-1" { "Password" }
                    input #password type="password" name="password" required minlength="8"
                          autocomplete="new-password" class="w-full border rounded px-3 py-2";
                }
                button type="submit"
                       class="w-full bg-indigo-600 text-white py-2 rounded hover:bg-indigo-700" {
                    "Sign up"
                }
                p class="text-sm text-gray-500 text-center" {
                    "Already have an account? " a href="/login" class="text-indigo-600 hover:underline" { "Log in" }
                }
            }
        },
    )
}

#[post("/signup")]
pub async fn signup(
    session: Session,
    mut db: Db,
    Form(form): Form<SignupForm>,
) -> AutumnResult<Redirect> {
    let email = form.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(AutumnError::unprocessable_msg("Enter a valid email address"));
    }
    if form.password.len() < 8 {
        return Err(AutumnError::unprocessable_msg(
            "Password must be at least 8 characters",
        ));
    }

    // Each account gets its own isolated tenant; email is the unique identifier
    // (already enforced by the UNIQUE constraint on users.email).
    let tenant_id = email.clone();
    let password_hash = hash_password(&form.password).await?;

    let user: User = diesel::insert_into(users::table)
        .values(&NewUser {
            email,
            password_hash,
            tenant_id,
        })
        .returning(User::as_returning())
        .get_result(&mut *db)
        .await
        // A duplicate email hits the UNIQUE constraint; surface the same generic
        // message a failed login does so the form does not enumerate accounts.
        .map_err(|_| AutumnError::unprocessable_msg("Could not create account"))?;

    establish_session(&session, &user).await;
    Ok(Redirect::to("/dashboard"))
}

// ── Login ────────────────────────────────────────────────────────────────────

#[get("/login")]
pub async fn login_form() -> Markup {
    layout(
        "Log in",
        false,
        html! {
            h1 class="text-2xl font-bold mb-6" { "Log in" }
            form action="/login" method="post" class="space-y-4 bg-white rounded-lg shadow p-6 max-w-md" {
                div {
                    label for="email" class="block text-sm font-medium mb-1" { "Email" }
                    input #email type="email" name="email" required autocomplete="email"
                          class="w-full border rounded px-3 py-2";
                }
                div {
                    label for="password" class="block text-sm font-medium mb-1" { "Password" }
                    input #password type="password" name="password" required
                          autocomplete="current-password" class="w-full border rounded px-3 py-2";
                }
                button type="submit"
                       class="w-full bg-indigo-600 text-white py-2 rounded hover:bg-indigo-700" {
                    "Log in"
                }
                p class="text-sm text-gray-500 text-center" {
                    "Need an account? " a href="/signup" class="text-indigo-600 hover:underline" { "Sign up" }
                }
            }
        },
    )
}

#[post("/login")]
pub async fn login(
    session: Session,
    mut db: Db,
    Form(form): Form<LoginForm>,
) -> AutumnResult<Redirect> {
    let email = form.email.trim().to_lowercase();

    let user: Option<User> = users::table
        .filter(users::email.eq(&email))
        .select(User::as_select())
        .first(&mut *db)
        .await
        .optional()?;

    let invalid = || AutumnError::unauthorized_msg("Invalid email or password");
    // Always run a bcrypt verification so the response time is constant whether
    // or not the email exists — prevents a timing side-channel that reveals
    // which accounts are registered.
    let user = match user {
        Some(u) => u,
        None => {
            let _ = verify_password(&form.password, DUMMY_HASH).await;
            return Err(invalid());
        }
    };
    if !verify_password(&form.password, &user.password_hash).await? {
        return Err(invalid());
    }

    establish_session(&session, &user).await;
    Ok(Redirect::to("/dashboard"))
}

// ── Logout ───────────────────────────────────────────────────────────────────

#[post("/logout")]
pub async fn logout(session: Session) -> Redirect {
    // Clear the session contents and rotate the id so the old cookie cannot be
    // replayed.
    session.clear().await;
    session.rotate_id().await;
    Redirect::to("/")
}

/// Log a user in: rotate the session id (prevents fixation) and record the
/// account + tenant the rest of the app scopes to.
async fn establish_session(session: &Session, user: &User) {
    session.rotate_id().await;
    session.insert("user_id", user.id.to_string()).await;
    session.insert("tenant_id", &user.tenant_id).await;
}
