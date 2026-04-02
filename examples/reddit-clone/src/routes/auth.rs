//! Authentication routes — register, login, logout.
//!
//! Demonstrates: Session extractor, password hashing (bcrypt),
//! session.insert / session.destroy, CsrfToken, form handling.

use autumn_web::auth::{hash_password, verify_password};
use autumn_web::extract::Path;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::{NewUser, User};
use crate::schema::users;

use super::layout::{layout, redirect_to};

// ── Register ───────────────────────────────────────────────────

#[get("/register")]
pub async fn register_form(csrf: CsrfToken) -> Markup {
    layout(
        "Sign Up",
        None,
        html! {
            div class="max-w-md mx-auto" {
                h1 class="text-2xl font-bold mb-6" { "Create an Account" }
                form action="/register" method="post" class="space-y-4 bg-white rounded-lg shadow p-6" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    div {
                        label for="username" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Username"
                        }
                        input type="text" id="username" name="username" required
                              autocomplete="username"
                              placeholder="cool_rustacean"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    div {
                        label for="password" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Password"
                        }
                        input type="password" id="password" name="password" required
                              autocomplete="new-password"
                              minlength="6"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    button type="submit"
                           class="w-full bg-orange-500 text-white py-2 rounded font-medium \
                                  hover:bg-orange-600 transition-colors" {
                        "Sign Up"
                    }
                    p class="text-center text-sm text-gray-500" {
                        "Already have an account? "
                        a href="/login" class="text-orange-600 hover:underline" { "Log in" }
                    }
                }
            }
        },
    )
}

#[derive(serde::Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub password: String,
}

#[post("/register")]
pub async fn register(
    mut db: Db,
    session: Session,
    form: Form<RegisterForm>,
) -> AutumnResult<Markup> {
    let username = form.0.username.trim().to_lowercase();
    let password = form.0.password;

    if username.len() < 2 || username.len() > 32 {
        return Err(AutumnError::unprocessable_msg(
            "Username must be 2-32 characters",
        ));
    }
    if password.len() < 6 {
        return Err(AutumnError::unprocessable_msg(
            "Password must be at least 6 characters",
        ));
    }

    // Check if username already taken
    let existing: i64 = users::table
        .filter(users::username.eq(&username))
        .count()
        .get_result(&mut *db)
        .await?;

    if existing > 0 {
        return Err(AutumnError::unprocessable_msg("Username already taken"));
    }

    let hashed = hash_password(&password)?;
    let new_user = NewUser {
        username: username.clone(),
        password_hash: hashed,
    };

    let user: User = diesel::insert_into(users::table)
        .values(&new_user)
        .returning(User::as_returning())
        .get_result(&mut *db)
        .await?;

    // Log in immediately after registration
    session.rotate_id().await;
    session.insert("user_id", user.id.to_string()).await;
    session.insert("username", &user.username).await;
    session.insert("role", &user.role).await;

    Ok(redirect_to("/"))
}

// ── Login ──────────────────────────────────────────────────────

#[get("/login")]
pub async fn login_form(csrf: CsrfToken) -> Markup {
    layout(
        "Log In",
        None,
        html! {
            div class="max-w-md mx-auto" {
                h1 class="text-2xl font-bold mb-6" { "Log In" }
                form action="/login" method="post" class="space-y-4 bg-white rounded-lg shadow p-6" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    div {
                        label for="username" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Username"
                        }
                        input type="text" id="username" name="username" required
                              autocomplete="username"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    div {
                        label for="password" class="block text-sm font-medium text-gray-700 mb-1" {
                            "Password"
                        }
                        input type="password" id="password" name="password" required
                              autocomplete="current-password"
                              class="w-full border border-gray-300 rounded px-3 py-2 text-sm \
                                     focus:outline-none focus:ring-2 focus:ring-orange-400";
                    }
                    button type="submit"
                           class="w-full bg-orange-500 text-white py-2 rounded font-medium \
                                  hover:bg-orange-600 transition-colors" {
                        "Log In"
                    }
                    p class="text-center text-sm text-gray-500" {
                        "New here? "
                        a href="/register" class="text-orange-600 hover:underline" { "Create an account" }
                    }
                }
            }
        },
    )
}

#[derive(serde::Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

#[post("/login")]
pub async fn login(
    mut db: Db,
    session: Session,
    form: Form<LoginForm>,
) -> AutumnResult<Markup> {
    let username = form.0.username.trim().to_lowercase();

    let user: User = users::table
        .filter(users::username.eq(&username))
        .select(User::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::bad_request_msg("Invalid username or password"))?;

    if !verify_password(&form.0.password, &user.password_hash)? {
        return Err(AutumnError::bad_request_msg("Invalid username or password"));
    }

    // Rotate session ID to prevent session fixation
    session.rotate_id().await;
    session.insert("user_id", user.id.to_string()).await;
    session.insert("username", &user.username).await;
    session.insert("role", &user.role).await;

    Ok(redirect_to("/"))
}

// ── Logout ─────────────────────────────────────────────────────

#[post("/logout")]
pub async fn logout(session: Session) -> Markup {
    session.destroy().await;
    redirect_to("/")
}

// ── Profile ────────────────────────────────────────────────────

#[get("/u/{username}")]
pub async fn profile(
    Path(name): Path<String>,
    session: Session,
    mut db: Db,
) -> AutumnResult<Markup> {
    let current_user = session.get("username").await;

    let user: User = users::table
        .filter(users::username.eq(&name))
        .select(User::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg(format!("User u/{name} not found")))?;

    Ok(layout(
        &format!("u/{}", user.username),
        current_user.as_deref(),
        html! {
            div class="bg-white rounded-lg shadow p-6" {
                div class="flex items-center gap-4 mb-4" {
                    div class="w-16 h-16 bg-orange-100 text-orange-600 rounded-full \
                               flex items-center justify-center text-2xl font-bold" {
                        (user.username.chars().next().unwrap_or('?').to_uppercase().to_string())
                    }
                    div {
                        h1 class="text-2xl font-bold" { "u/" (user.username) }
                        p class="text-sm text-gray-500" {
                            (user.karma) " karma"
                            " \u{2022} joined "
                            (user.created_at.format("%b %d, %Y"))
                        }
                    }
                }
            }
        },
    ))
}
