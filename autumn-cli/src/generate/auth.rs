//! `autumn generate auth` — generate a complete browser authentication flow.
//!
//! Creates a User model, Diesel migration, auth route handlers (signup / login /
//! logout / account / forgot-password / reset-password), generated request-level
//! tests, and a documentation file — all as ordinary app-owned code that the
//! user can edit freely after generation.
//!
//! Security properties of the generated code:
//! - Passwords are hashed with bcrypt (cost=12) via `autumn_web::auth`.
//! - Reset tokens are random values; only SHA-256 digests are persisted.
//! - Duplicate signup and failed login return identical non-enumerating errors.
//! - Login and reset-password rotate the session ID (prevents session fixation).
//! - Logout destroys the session (old session cannot remain authenticated).

use std::path::Path;

use super::emit::Plan;
use super::model::plan_cargo_deps;
use super::naming::{pascal, pluralize, snake};
use super::schema_edit::{add_mod_declaration, append_schema_table, update_main_rs};
use super::{Flags, GenerateError, ensure_project_root, timestamp_now};

/// Extra Cargo dependencies the auth generator needs on top of the model deps.
const AUTH_EXTRA_DEPS: &[(&str, &str)] = &[
    ("maud", "{ version = \"0.27\", features = [\"axum\"] }"),
    ("sha2", "{ version = \"0.10\", features = [] }"),
    ("hex", "\"0.4\""),
    ("rand", "{ version = \"0.9\", features = [\"os_rng\"] }"),
];

/// Compute the file actions for `autumn generate auth`.
///
/// Pure planning step — no I/O happens here. Tests use this directly so they
/// can inspect the emitted file list and contents without touching the disk.
///
/// # Errors
/// Returns [`GenerateError::NotInProject`] when run outside an Autumn project
/// root, or [`GenerateError::InvalidName`] for a bad resource name.
pub fn plan_auth(
    project_root: &Path,
    name: &str,
    timestamp: &str,
) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;

    let pascal_name = pascal(name);
    let snake_name = snake(name);
    let table = pluralize(&snake_name);

    let mut plan = Plan::new(project_root);

    // ── Migration ──────────────────────────────────────────────────────────
    let mig_dir = project_root
        .join("migrations")
        .join(format!("{timestamp}_create_{table}"));
    plan.create(mig_dir.join("up.sql"), render_migration_up(&table));
    plan.create(mig_dir.join("down.sql"), render_migration_down(&table));

    // ── Model ──────────────────────────────────────────────────────────────
    let models_dir = project_root.join("src").join("models");
    plan.create(
        models_dir.join(format!("{snake_name}.rs")),
        render_model_file(&pascal_name, &snake_name, &table),
    );
    let model_mod_path = models_dir.join("mod.rs");
    plan.modify(
        model_mod_path.clone(),
        add_mod_declaration(&read_or_empty(&model_mod_path), &snake_name),
    );

    // ── src/schema.rs entry ────────────────────────────────────────────────
    // The generated model references `crate::schema::<table>`, so we must
    // emit a `diesel::table! { }` block just like `generate model` does.
    // Auth-specific fields (id and created_at are added automatically):
    //   email            String   → Text      NOT NULL
    //   password_digest  String   → Text      NOT NULL
    //   reset_token_digest         Option<String>         → Nullable<Text>
    //   reset_token_expires_at     Option<NaiveDateTime>  → Nullable<Timestamp>
    let auth_fields: Vec<super::dsl::Field> = [
        "email:String",
        "password_digest:String",
        "reset_token_digest:Option<String>",
        "reset_token_expires_at:Option<NaiveDateTime>",
    ]
    .iter()
    .map(|t| super::dsl::parse_field(t).expect("auth field tokens are always valid"))
    .collect();

    let schema_path = project_root.join("src").join("schema.rs");
    let schema_existing = read_or_empty(&schema_path);
    plan.modify(
        schema_path,
        append_schema_table(&schema_existing, &table, &auth_fields),
    );

    // ── Auth routes ────────────────────────────────────────────────────────
    let routes_dir = project_root.join("src").join("routes");
    plan.create(
        routes_dir.join("auth.rs"),
        render_routes_file(&pascal_name, &snake_name, &table),
    );
    let route_mod_path = routes_dir.join("mod.rs");
    plan.modify(
        route_mod_path.clone(),
        add_mod_declaration(&read_or_empty(&route_mod_path), "auth"),
    );

    // ── Generated tests ────────────────────────────────────────────────────
    let tests_dir = project_root.join("tests");
    plan.create(
        tests_dir.join("auth.rs"),
        render_tests_file(&pascal_name, &snake_name),
    );

    // ── Documentation ─────────────────────────────────────────────────────
    let docs_dir = project_root.join("docs").join("guide");
    plan.create(docs_dir.join("authentication.md"), render_docs_file(&pascal_name));

    // ── src/main.rs — module declarations + route registration ────────────
    let main_path = project_root.join("src").join("main.rs");
    let main_existing = std::fs::read_to_string(&main_path).map_err(|_| {
        GenerateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing {}", main_path.display()),
        ))
    })?;
    let entries = auth_route_entries();
    let updated = update_main_rs(&main_existing, &["models", "routes", "schema"], &entries);
    plan.modify(main_path, updated);

    // ── Cargo.toml deps ───────────────────────────────────────────────────
    let all_deps: Vec<(&str, &str)> = super::model::MODEL_DEPS
        .iter()
        .copied()
        .chain(AUTH_EXTRA_DEPS.iter().copied())
        .collect();
    plan_cargo_deps(&mut plan, project_root, &all_deps);

    Ok(plan)
}

/// CLI entry point for `autumn generate auth <Name>`.
pub fn run(name: &str, flags: Flags) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    let timestamp = timestamp_now();
    let plan = plan_auth(&cwd, name, &timestamp);
    match plan.and_then(|p| p.execute(flags)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

// ── Template rendering ────────────────────────────────────────────────────────

fn render_migration_up(table: &str) -> String {
    format!(
        "CREATE TABLE {table} (\n\
         \x20   id BIGSERIAL PRIMARY KEY,\n\
         \x20   email TEXT NOT NULL UNIQUE,\n\
         \x20   password_digest TEXT NOT NULL,\n\
         \x20   reset_token_digest TEXT NULL,\n\
         \x20   reset_token_expires_at TIMESTAMP NULL,\n\
         \x20   created_at TIMESTAMP NOT NULL DEFAULT NOW()\n\
         );\n"
    )
}

fn render_migration_down(table: &str) -> String {
    format!("DROP TABLE {table};\n")
}

fn render_model_file(pascal_name: &str, _snake_name: &str, table: &str) -> String {
    format!(
        r#"//! Generated by `autumn generate auth`.
//!
//! Edit freely — once generated, this is ordinary user code.
//! Security note: never store raw passwords or reset tokens here, only digests.

use chrono::NaiveDateTime;
use diesel::prelude::*;

use crate::schema::{table};

#[autumn_web::model]
pub struct {pascal_name} {{
    pub id: i64,
    pub email: String,
    pub password_digest: String,
    pub reset_token_digest: Option<String>,
    pub reset_token_expires_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
}}
"#
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "Single auth-routes template — splitting fragments makes the template harder to read."
)]
fn render_routes_file(pascal_name: &str, snake_name: &str, table: &str) -> String {
    format!(
        r#"//! Generated by `autumn generate auth`.
//!
//! Complete browser authentication flow. Edit freely — once generated,
//! this is ordinary user code.
//!
//! Security properties:
//! - Passwords are hashed with bcrypt via `autumn_web::auth`.
//! - Reset tokens are 32-byte random values; only the SHA-256 digest is stored.
//! - Duplicate signup and failed login return identical non-enumerating errors.
//! - Login and reset-password rotate the session ID to prevent fixation.
//! - Logout destroys the session so the old session cannot remain authenticated.

use autumn_web::auth::{{hash_password, verify_password}};
use autumn_web::extract::Query;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::Deserialize;

use crate::models::{snake_name}::{{New{pascal_name}, {pascal_name}}};
use crate::schema::{table};

// ── Layout helpers ────────────────────────────────────────────────────────────

fn layout(title: &str, content: Markup) -> Markup {{
    html! {{
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {{
            head {{
                meta charset="utf-8";
                title {{ (title) }}
            }}
            body {{ (content) }}
        }}
    }}
}}

fn redirect_to(url: &str) -> Markup {{
    html! {{
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html {{
            head {{
                meta http-equiv="refresh" content=(format!("0;url={{url}}"));
            }}
            body {{ p {{ "Redirecting…" }} }}
        }}
    }}
}}

// ── Signup ────────────────────────────────────────────────────────────────────

/// `GET /signup` — render the signup form.
#[get("/signup")]
pub async fn signup_form(csrf: CsrfToken) -> AutumnResult<Markup> {{
    Ok(layout("Sign Up", html! {{
        h1 {{ "Create an Account" }}
        form action="/signup" method="post" {{
            input type="hidden" name="_csrf" value=(csrf.token());
            div {{
                label {{ "Email" }}
                input type="email" name="email" required autocomplete="email";
            }}
            div {{
                label {{ "Password (8+ characters)" }}
                input type="password" name="password" required
                      autocomplete="new-password" minlength="8";
            }}
            button type="submit" {{ "Sign Up" }}
        }}
        p {{ a href="/login" {{ "Already have an account? Log in" }} }}
    }})))
}}

#[derive(Deserialize)]
pub struct SignupForm {{
    pub email: String,
    pub password: String,
}}

/// `POST /signup` — create a new account and start a session.
///
/// Non-enumerating: returns the same error whether the email is taken or invalid
/// so callers cannot learn which addresses are registered.
#[post("/signup")]
pub async fn signup(
    mut db: Db,
    session: Session,
    Form(form): Form<SignupForm>,
) -> AutumnResult<Markup> {{
    let email = form.email.trim().to_lowercase();
    if !email.contains('@') {{
        return Err(AutumnError::unprocessable_msg("Invalid email address."));
    }}
    if form.password.len() < 8 {{
        return Err(AutumnError::unprocessable_msg(
            "Password must be at least 8 characters.",
        ));
    }}

    let password_digest = hash_password(&form.password).await?;
    let new_{snake_name} = New{pascal_name} {{
        email: email.clone(),
        password_digest,
        reset_token_digest: None,
        reset_token_expires_at: None,
    }};

    let result: Result<{pascal_name}, _> = diesel::insert_into({table}::table)
        .values(&new_{snake_name})
        .returning({pascal_name}::as_returning())
        .get_result(&mut *db)
        .await;

    let {snake_name} = result.map_err(|_| {{
        AutumnError::unprocessable_msg(
            "Unable to create account. Please try a different email.",
        )
    }})?;

    session.rotate_id().await;
    session.insert("{snake_name}_id", {snake_name}.id.to_string()).await;
    session.insert("{snake_name}_email", &{snake_name}.email).await;
    Ok(redirect_to("/account"))
}}

// ── Login ─────────────────────────────────────────────────────────────────────

/// `GET /login` — render the login form.
#[get("/login")]
pub async fn login_form(csrf: CsrfToken) -> AutumnResult<Markup> {{
    Ok(layout("Log In", html! {{
        h1 {{ "Log In" }}
        form action="/login" method="post" {{
            input type="hidden" name="_csrf" value=(csrf.token());
            div {{
                label {{ "Email" }}
                input type="email" name="email" required autocomplete="email";
            }}
            div {{
                label {{ "Password" }}
                input type="password" name="password" required
                      autocomplete="current-password";
            }}
            button type="submit" {{ "Log In" }}
        }}
        p {{ a href="/signup" {{ "New here? Create an account" }} }}
        p {{ a href="/forgot-password" {{ "Forgot your password?" }} }}
    }})))
}}

#[derive(Deserialize)]
pub struct LoginForm {{
    pub email: String,
    pub password: String,
}}

/// `POST /login` — verify credentials and start a session.
///
/// Non-enumerating: returns the same error for unknown email and wrong password
/// so callers cannot learn which addresses are registered.
#[post("/login")]
pub async fn login(
    mut db: Db,
    session: Session,
    Form(form): Form<LoginForm>,
) -> AutumnResult<Markup> {{
    let email = form.email.trim().to_lowercase();
    let auth_err = || AutumnError::unprocessable_msg("Invalid email or password.");

    let {snake_name}: {pascal_name} = {table}::table
        .filter({table}::email.eq(&email))
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| auth_err())?;

    if !verify_password(&form.password, &{snake_name}.password_digest).await? {{
        return Err(auth_err());
    }}

    session.rotate_id().await;
    session.insert("{snake_name}_id", {snake_name}.id.to_string()).await;
    session.insert("{snake_name}_email", &{snake_name}.email).await;
    Ok(redirect_to("/account"))
}}

// ── Logout ────────────────────────────────────────────────────────────────────

/// `POST /logout` — destroy the session and redirect to the login page.
///
/// Destroying (not just clearing) the session ensures an old session cookie
/// cannot be replayed after logout.
#[post("/logout")]
pub async fn logout(session: Session) -> AutumnResult<Markup> {{
    session.destroy().await;
    Ok(redirect_to("/login"))
}}

// ── Account (protected example route) ────────────────────────────────────────

/// `GET /account` — current-account profile placeholder. Requires authentication.
///
/// This is a protected-route example: the `#[secured]` attribute rejects
/// anonymous requests before the handler body runs.
#[secured]
#[get("/account")]
pub async fn account(session: Session, mut db: Db) -> AutumnResult<Markup> {{
    let {snake_name}_id: i64 = session
        .get::<String>("{snake_name}_id")
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;

    let {snake_name}: {pascal_name} = {table}::table
        .find({snake_name}_id)
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Account not found."))?;

    Ok(layout("Your Account", html! {{
        h1 {{ "Your Account" }}
        p {{ "Email: " ({snake_name}.email) }}
        form action="/logout" method="post" {{
            button type="submit" {{ "Log Out" }}
        }}
    }})))
}}

// ── Forgot Password ───────────────────────────────────────────────────────────

/// `GET /forgot-password` — render the forgot-password form.
#[get("/forgot-password")]
pub async fn forgot_password_form(csrf: CsrfToken) -> AutumnResult<Markup> {{
    Ok(layout("Forgot Password", html! {{
        h1 {{ "Forgot Your Password?" }}
        form action="/forgot-password" method="post" {{
            input type="hidden" name="_csrf" value=(csrf.token());
            div {{
                label {{ "Email" }}
                input type="email" name="email" required autocomplete="email";
            }}
            button type="submit" {{ "Send Reset Link" }}
        }}
    }})))
}}

#[derive(Deserialize)]
pub struct ForgotPasswordForm {{
    pub email: String,
}}

/// `POST /forgot-password` — generate a reset token and email a reset link.
///
/// Non-enumerating: always returns the same confirmation page whether or not
/// the email address is registered, so callers cannot learn which addresses
/// exist. Requires Autumn mail to be configured; fails with a clear message
/// identifying the missing mail configuration if it is not.
#[post("/forgot-password")]
pub async fn forgot_password(
    mut db: Db,
    mailer: Mailer,
    Form(form): Form<ForgotPasswordForm>,
) -> AutumnResult<Markup> {{
    let email = form.email.trim().to_lowercase();

    // Non-enumerating: silently skip unknown addresses.
    let maybe_{snake_name}: Option<{pascal_name}> = {table}::table
        .filter({table}::email.eq(&email))
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .ok();

    if let Some({snake_name}) = maybe_{snake_name} {{
        let raw_token = generate_reset_token();
        let token_digest = sha256_hex(&raw_token);
        let expires_at =
            chrono::Utc::now().naive_utc() + chrono::Duration::hours(2);

        diesel::update({table}::table.find({snake_name}.id))
            .set((
                {table}::reset_token_digest.eq(Some(&token_digest)),
                {table}::reset_token_expires_at.eq(Some(expires_at)),
            ))
            .execute(&mut *db)
            .await?;

        send_reset_email(&mailer, &{snake_name}.email, &raw_token).await?;
    }}

    Ok(layout("Check Your Email", html! {{
        h1 {{ "Check Your Email" }}
        p {{
            "If that address is registered you'll receive a reset link shortly."
        }}
        p {{ a href="/login" {{ "Back to login" }} }}
    }})))
}}

// ── Reset Password ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ResetPasswordQuery {{
    pub token: String,
}}

/// `GET /reset-password?token=<raw>` — render the reset-password form.
#[get("/reset-password")]
pub async fn reset_password_form(
    Query(query): Query<ResetPasswordQuery>,
    csrf: CsrfToken,
) -> AutumnResult<Markup> {{
    Ok(layout("Reset Password", html! {{
        h1 {{ "Set a New Password" }}
        form action="/reset-password" method="post" {{
            input type="hidden" name="_csrf" value=(csrf.token());
            input type="hidden" name="token" value=(query.token);
            div {{
                label {{ "New Password (8+ characters)" }}
                input type="password" name="password" required
                      autocomplete="new-password" minlength="8";
            }}
            button type="submit" {{ "Set New Password" }}
        }}
    }})))
}}

#[derive(Deserialize)]
pub struct ResetPasswordForm {{
    pub token: String,
    pub password: String,
}}

/// `POST /reset-password` — verify the reset token and update the password.
///
/// The token is compared via its stored digest (constant-time via `sha2`).
/// On success the session is rotated, invalidating any prior authenticated
/// state.
#[post("/reset-password")]
pub async fn reset_password(
    mut db: Db,
    session: Session,
    Form(form): Form<ResetPasswordForm>,
) -> AutumnResult<Markup> {{
    if form.password.len() < 8 {{
        return Err(AutumnError::unprocessable_msg(
            "Password must be at least 8 characters.",
        ));
    }}

    let token_digest = sha256_hex(&form.token);
    let now = chrono::Utc::now().naive_utc();

    let {snake_name}: {pascal_name} = {table}::table
        .filter({table}::reset_token_digest.eq(Some(&token_digest)))
        .filter({table}::reset_token_expires_at.gt(now))
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| {{
            AutumnError::unprocessable_msg("Invalid or expired reset link.")
        }})?;

    let new_digest = hash_password(&form.password).await?;
    diesel::update({table}::table.find({snake_name}.id))
        .set((
            {table}::password_digest.eq(&new_digest),
            {table}::reset_token_digest.eq(None::<String>),
            {table}::reset_token_expires_at.eq(None::<chrono::NaiveDateTime>),
        ))
        .execute(&mut *db)
        .await?;

    // Rotate session to invalidate any previous authenticated state.
    session.rotate_id().await;
    session.insert("{snake_name}_id", {snake_name}.id.to_string()).await;
    session.insert("{snake_name}_email", &{snake_name}.email).await;
    Ok(redirect_to("/account"))
}}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Generate a 32-byte cryptographically-random reset token, hex-encoded.
fn generate_reset_token() -> String {{
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}}

/// SHA-256 hex digest of `input` using the `sha2` crate.
fn sha256_hex(input: &str) -> String {{
    use sha2::{{Digest, Sha256}};
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)
}}

/// Send a password-reset email via the Autumn mailer.
///
/// # Errors
/// Returns a clear `AutumnError::internal` message when mail is not configured
/// (`transport = "disabled"`) or when the send itself fails.
async fn send_reset_email(mailer: &Mailer, to: &str, token: &str) -> AutumnResult<()> {{
    if mailer.is_disabled() {{
        return Err(AutumnError::internal_msg(
            "Password reset requires mail to be configured. \
             Set [mail] transport in autumn.toml (e.g. transport = \"smtp\"). \
             The forgot-password feature is unavailable until mail is set up."
                .to_owned(),
        ));
    }}
    let reset_url = format!("/reset-password?token={{token}}");
    let mail = Mail::builder()
        .to(to.to_owned())
        .subject("Reset your password")
        .html(html! {{
            p {{ "Click the link below to reset your password." }}
            p {{ "This link expires in 2 hours." }}
            p {{ a href=(&reset_url) {{ "Reset Password" }} }}
            p {{ "If you did not request this, you can safely ignore this email." }}
        }})
        .text(format!(
            "Reset your password: {{reset_url}}\n\
             This link expires in 2 hours.\n\
             If you did not request this you can safely ignore this email."
        ))
        .build()
        .map_err(|e| {{
            AutumnError::internal_msg(format!(
                "Failed to build password-reset email: {{e}}"
            ))
        }})?;
    mailer.send(mail).await.map_err(|e| {{
        AutumnError::internal_msg(format!(
            "Failed to send password-reset email: {{e}}"
        ))
    }})
}}
"#
    )
}

fn render_tests_file(_pascal_name: &str, _snake_name: &str) -> String {
    format!(
        r#"//! Request-level smoke tests generated by `autumn generate auth`.
//!
//! These tests run against a live server started with `AUTUMN_TEST_BASE_URL`.
//! In CI, start the app, set the env var, and run `cargo test`.
//!
//! Each test uses a raw TCP connection to avoid adding an HTTP client dep;
//! replace with your preferred HTTP client once it is in `Cargo.toml`.

use std::io::{{Read, Write}};
use std::net::TcpStream;

fn base_url() -> Option<String> {{
    std::env::var("AUTUMN_TEST_BASE_URL").ok()
}}

fn host_port(base: &str) -> String {{
    base.trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_owned()
}}

fn get(base: &str, path: &str) -> String {{
    let hp = host_port(base);
    let mut stream =
        TcpStream::connect(&hp).unwrap_or_else(|_| panic!("cannot connect to {{base}}"));
    let req = format!("GET {{path}} HTTP/1.1\r\nHost: {{hp}}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).expect("write failed");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read failed");
    resp
}}

fn post_form(base: &str, path: &str, body: &str, cookie: &str) -> String {{
    let hp = host_port(base);
    let mut stream =
        TcpStream::connect(&hp).unwrap_or_else(|_| panic!("cannot connect to {{base}}"));
    let req = format!(
        "POST {{path}} HTTP/1.1\r\n\
         Host: {{hp}}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {{}}\r\n\
         Cookie: {{cookie}}\r\n\
         Connection: close\r\n\r\n\
         {{body}}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write failed");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read failed");
    resp
}}

#[test]
fn auth_signup_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/signup");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /signup did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_login_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/login");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /login did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_logout_redirects() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = post_form(&base, "/logout", "", "");
    assert!(
        resp.contains("HTTP/1.1 30") || resp.contains("HTTP/1.0 30"),
        "POST /logout did not redirect:\n{{resp}}"
    );
}}

#[test]
fn auth_forgot_password_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/forgot-password");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /forgot-password did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_reset_password_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/reset-password?token=dummy");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /reset-password did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_account_rejects_anonymous() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    // Without a session cookie, /account must reject with 401 or redirect.
    let resp = get(&base, "/account");
    let is_rejected = resp.contains("HTTP/1.1 401")
        || resp.contains("HTTP/1.0 401")
        || resp.contains("HTTP/1.1 30")
        || resp.contains("HTTP/1.0 30");
    assert!(
        is_rejected,
        "GET /account should reject anonymous requests (expected 401 or redirect):\n{{resp}}"
    );
}}
"#
    )
}

fn render_docs_file(pascal_name: &str) -> String {
    format!(
        r#"# Authentication Guide

Generated by `autumn generate auth`. Edit freely.

## Overview

This guide documents the browser-session authentication flow generated for
your Autumn application. The generated code handles signup, login, logout,
account profile, and password reset using Autumn's built-in session, CSRF,
password hashing, and mail primitives.

## Generated Routes

| Method | Path | Handler | Auth |
|--------|------|---------|------|
| GET | `/signup` | `signup_form` | Public |
| POST | `/signup` | `signup` | Public |
| GET | `/login` | `login_form` | Public |
| POST | `/login` | `login` | Public |
| POST | `/logout` | `logout` | Any |
| GET | `/account` | `account` | **Required** |
| GET | `/forgot-password` | `forgot_password_form` | Public |
| POST | `/forgot-password` | `forgot_password` | Public |
| GET | `/reset-password` | `reset_password_form` | Public |
| POST | `/reset-password` | `reset_password` | Public |

## Security Properties

- **Passwords**: Hashed with bcrypt (cost 12) via `autumn_web::auth::hash_password`.
  Raw passwords are never logged or stored.
- **Reset tokens**: 32-byte random values generated with `OsRng`; only the
  SHA-256 digest is stored in `reset_token_digest`. The raw token is sent by
  email only and expires after 2 hours.
- **Non-enumeration**: Duplicate signup, failed login, and forgot-password
  submissions for unknown addresses all return responses that do not reveal
  whether an email address is registered.
- **Session fixation**: Login and password-reset rotate the session ID
  (`session.rotate_id()`).
- **Session invalidation**: Logout calls `session.destroy()` so an old session
  cookie cannot be replayed.
- **Protected routes**: The `/account` route uses `#[secured]` to reject
  unauthenticated requests before the handler runs.

## Development Mail Flow

In development, configure file-based mail capture in `autumn.toml`:

```toml
[mail]
transport = "file"
from = "Your App <noreply@yourapp.dev>"
```

Sent emails are written as `.eml` files to `target/mail/`. Open them with any
email client to preview the password-reset link.

If mail is not configured (transport = "disabled"), the forgot-password handler
returns a clear error message identifying the missing `[mail]` configuration
and the password-reset feature affected.

## Customization Points

- **Validation**: Add stricter email / password rules to `signup` and
  `reset_password` in `src/routes/auth.rs`.
- **Session keys**: Change the session key names (`{snake_name}_id`,
  `{snake_name}_email`) to match your application's conventions.
- **Redirect targets**: Adjust `redirect_to("/account")` calls to send users
  to the right page after login/signup/reset.
- **Email templates**: Customise the `send_reset_email` function to match your
  brand.
- **{pascal_name} fields**: Add display-name, avatar, or role fields to the
  `{pascal_name}` model and a new migration.

## When to Choose This Flow vs. Alternatives

| Scenario | Recommendation |
|----------|---------------|
| Browser-based web app | ✅ This generated flow |
| Mobile / CLI / third-party API clients | API tokens (`autumn generate token` — see [#520]) |
| Social login (Google, GitHub, …) | OAuth2/OIDC (S-059) |
| Enterprise / SSO | SAML / enterprise IdP (future) |

## Quick Start

```sh
autumn new myapp
cd myapp
autumn generate auth {pascal_name}
autumn migrate
autumn dev
```

Then open <http://localhost:3000/signup> to create your first account.
"#,
        snake_name = pascal_name.to_lowercase(),
        pascal_name = pascal_name,
    )
}

fn auth_route_entries() -> Vec<String> {
    vec![
        "routes::auth::signup_form".to_owned(),
        "routes::auth::signup".to_owned(),
        "routes::auth::login_form".to_owned(),
        "routes::auth::login".to_owned(),
        "routes::auth::logout".to_owned(),
        "routes::auth::account".to_owned(),
        "routes::auth::forgot_password_form".to_owned(),
        "routes::auth::forgot_password".to_owned(),
        "routes::auth::reset_password_form".to_owned(),
        "routes::auth::reset_password".to_owned(),
    ]
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn project_with_main() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(
            tmp.path().join("src/main.rs"),
            "use autumn_web::prelude::*;\n\n\
             #[autumn_web::main]\n\
             async fn main() {\n\
             \x20   autumn_web::app().routes(routes![]).run().await;\n\
             }\n",
        )
        .unwrap();
        tmp
    }

    // ── Plan structure ──────────────────────────────────────────────────────

    #[test]
    fn plan_auth_creates_expected_files() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        let paths: Vec<String> = plan
            .actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(tmp.path())
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();

        for expected in [
            "src/models/user.rs",
            "src/models/mod.rs",
            "src/schema.rs",
            "migrations/20260508000000_create_users/up.sql",
            "migrations/20260508000000_create_users/down.sql",
            "src/routes/auth.rs",
            "src/routes/mod.rs",
            "tests/auth.rs",
            "docs/guide/authentication.md",
            "src/main.rs",
        ] {
            assert!(
                paths.iter().any(|p| p == expected),
                "missing expected action for {expected}; got {paths:?}"
            );
        }
    }

    #[test]
    fn plan_auth_errors_when_not_in_project() {
        let tmp = TempDir::new().unwrap();
        let err = plan_auth(tmp.path(), "User", "20260508000000").unwrap_err();
        assert!(matches!(err, GenerateError::NotInProject));
    }

    // ── Migration SQL ───────────────────────────────────────────────────────

    #[test]
    fn migration_up_sql_creates_users_table_with_digest_columns() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let up = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000000_create_users/up.sql"),
        )
        .unwrap();
        assert!(up.contains("CREATE TABLE users"), "missing CREATE TABLE: {up}");
        assert!(up.contains("email"), "missing email column: {up}");
        assert!(up.contains("password_digest"), "missing password_digest: {up}");
        assert!(up.contains("reset_token_digest"), "missing reset_token_digest: {up}");
        assert!(up.contains("reset_token_expires_at"), "missing reset_token_expires_at: {up}");
        assert!(up.contains("UNIQUE"), "email column must be UNIQUE: {up}");
        assert!(!up.contains("password TEXT"), "raw password must never be stored: {up}");
        assert!(!up.contains("reset_token TEXT"), "raw reset_token must never be stored: {up}");
    }

    #[test]
    fn migration_down_sql_drops_users_table() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let down = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000000_create_users/down.sql"),
        )
        .unwrap();
        assert!(down.contains("DROP TABLE users"), "missing DROP TABLE: {down}");
    }

    // ── schema.rs ───────────────────────────────────────────────────────────

    #[test]
    fn schema_rs_contains_diesel_table_for_auth_table() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(schema.contains("users (id)"), "schema missing table block: {schema}");
        assert!(schema.contains("email -> Text"), "schema missing email column: {schema}");
        assert!(
            schema.contains("password_digest -> Text"),
            "schema missing password_digest: {schema}"
        );
        assert!(
            schema.contains("reset_token_digest -> Nullable<Text>"),
            "schema missing nullable reset_token_digest: {schema}"
        );
        assert!(
            schema.contains("reset_token_expires_at -> Nullable<Timestamp>"),
            "schema missing nullable reset_token_expires_at: {schema}"
        );
    }

    // ── Model file ──────────────────────────────────────────────────────────

    #[test]
    fn model_file_contains_struct_and_digest_fields() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let model = fs::read_to_string(tmp.path().join("src/models/user.rs")).unwrap();
        assert!(model.contains("pub struct User"), "missing struct: {model}");
        assert!(model.contains("pub email: String"), "missing email: {model}");
        assert!(model.contains("pub password_digest: String"), "missing password_digest: {model}");
        assert!(
            model.contains("pub reset_token_digest: Option<String>"),
            "reset_token_digest must be nullable: {model}"
        );
        assert!(!model.contains("pub password:"), "raw password must not be a field: {model}");
        assert!(
            !model.contains("pub reset_token:"),
            "raw reset_token must not be a field: {model}"
        );
    }

    #[test]
    fn model_mod_rs_declares_module() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let mod_rs = fs::read_to_string(tmp.path().join("src/models/mod.rs")).unwrap();
        assert!(mod_rs.contains("pub mod user;"), "missing pub mod user: {mod_rs}");
    }

    // ── Routes file ─────────────────────────────────────────────────────────

    #[test]
    fn routes_file_contains_all_handlers() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        for needle in [
            "pub async fn signup_form",
            "pub async fn signup",
            "pub async fn login_form",
            "pub async fn login",
            "pub async fn logout",
            "pub async fn account",
            "pub async fn forgot_password_form",
            "pub async fn forgot_password",
            "pub async fn reset_password_form",
            "pub async fn reset_password",
        ] {
            assert!(routes.contains(needle), "routes missing handler: {needle}");
        }
    }

    #[test]
    fn routes_file_uses_session_invalidation_on_logout() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("session.destroy"),
            "logout must destroy the session: {routes}"
        );
    }

    #[test]
    fn routes_file_rotates_session_on_login() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("session.rotate_id"),
            "login must rotate the session ID to prevent fixation: {routes}"
        );
    }

    #[test]
    fn routes_file_account_is_protected() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("#[secured]"),
            "account route must use #[secured] for protection: {routes}"
        );
    }

    #[test]
    fn routes_mod_rs_declares_auth_module() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let mod_rs = fs::read_to_string(tmp.path().join("src/routes/mod.rs")).unwrap();
        assert!(mod_rs.contains("pub mod auth;"), "missing pub mod auth: {mod_rs}");
    }

    #[test]
    fn routes_file_forgot_password_checks_mailer_is_disabled() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("mailer.is_disabled()"),
            "forgot_password must guard against disabled mail transport: {routes}"
        );
        assert!(
            routes.contains("mailer.send(mail).await"),
            "forgot_password must use async mailer.send(): {routes}"
        );
    }

    // ── Generated tests ─────────────────────────────────────────────────────

    #[test]
    fn tests_file_covers_all_required_flows() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let tests = fs::read_to_string(tmp.path().join("tests/auth.rs")).unwrap();
        for needle in [
            "auth_signup_returns_200",
            "auth_login_returns_200",
            "auth_logout_redirects",
            "auth_forgot_password_returns_200",
            "auth_reset_password_returns_200",
            "auth_account_rejects_anonymous",
        ] {
            assert!(tests.contains(needle), "tests missing flow: {needle}");
        }
    }

    // ── main.rs registration ────────────────────────────────────────────────

    #[test]
    fn main_rs_registers_auth_routes() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        for entry in [
            "routes::auth::signup_form",
            "routes::auth::signup",
            "routes::auth::login_form",
            "routes::auth::login",
            "routes::auth::logout",
            "routes::auth::account",
            "routes::auth::forgot_password_form",
            "routes::auth::forgot_password",
            "routes::auth::reset_password_form",
            "routes::auth::reset_password",
        ] {
            assert!(main.contains(entry), "main.rs missing route entry: {entry}");
        }
    }

    #[test]
    fn main_rs_declares_models_and_routes_mods() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(main.contains("mod models;"), "main.rs missing mod models: {main}");
        assert!(main.contains("mod routes;"), "main.rs missing mod routes: {main}");
    }

    // ── Dry run ─────────────────────────────────────────────────────────────

    #[test]
    fn dry_run_writes_no_files() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags { dry_run: true, force: false }).unwrap();
        assert!(
            !tmp.path().join("src/models/user.rs").exists(),
            "dry run must not create model file"
        );
        assert!(
            !tmp.path().join("src/routes/auth.rs").exists(),
            "dry run must not create routes file"
        );
    }

    // ── Non-default model name ──────────────────────────────────────────────

    #[test]
    fn plan_auth_supports_custom_model_name() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "Account", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        assert!(
            tmp.path().join("src/models/account.rs").exists(),
            "model file should use snake_case of given name"
        );
        let model = fs::read_to_string(tmp.path().join("src/models/account.rs")).unwrap();
        assert!(model.contains("pub struct Account"), "struct name should match given name");
    }
}
