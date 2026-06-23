//! Integration tests for the SaaS starter.
//!
//! ```text
//! cargo test                                   # smoke tests (instant, no Docker)
//! cargo test -- --include-ignored --test-threads=1   # full flow (needs Docker)
//! ```
//!
//! The ignored tests start a Postgres testcontainer and drive the real signup →
//! login → tenant-scoped dashboard flow, then prove one tenant cannot see
//! another's data.

use autumn_web::config::AutumnConfig;
use autumn_web::prelude::*;
use autumn_web::test::{TestApp, TestClient, TestDb};

use saas::routes;

fn app_routes() -> Vec<autumn_web::Route> {
    routes![
        saas::index,
        routes::auth::signup_form,
        routes::auth::signup,
        routes::auth::login_form,
        routes::auth::login,
        routes::auth::logout,
        routes::dashboard::dashboard,
        routes::dashboard::create_project,
    ]
}

// ── Smoke test (no Docker) ───────────────────────────────────────────────────

/// The login page renders without a database — proving the app wires up.
#[tokio::test]
async fn login_page_renders() {
    let client = TestApp::new().routes(app_routes()).build();
    client
        .get("/login")
        .send()
        .await
        .assert_ok()
        .assert_body_contains("Log in");
}

// ── Full flow (requires Docker) ──────────────────────────────────────────────

/// Create the schema and return a CSRF-disabled, DB-backed client.
async fn db_client() -> TestClient {
    let db = TestDb::shared().await;
    db.execute_sql(
        "CREATE TABLE IF NOT EXISTS users (
            id            BIGSERIAL PRIMARY KEY,
            email         TEXT      NOT NULL UNIQUE,
            password_hash TEXT      NOT NULL,
            tenant_id     TEXT      NOT NULL,
            created_at    TIMESTAMP NOT NULL DEFAULT NOW()
        )",
    )
    .await;
    db.execute_sql(
        "CREATE TABLE IF NOT EXISTS projects (
            id         BIGSERIAL PRIMARY KEY,
            tenant_id  TEXT      NOT NULL,
            name       TEXT      NOT NULL,
            created_at TIMESTAMP NOT NULL DEFAULT NOW()
        )",
    )
    .await;
    db.execute_sql("TRUNCATE users, projects RESTART IDENTITY").await;

    // The forms post normally; disable CSRF so the test does not have to scrape
    // a token out of the rendered HTML.
    let mut config = AutumnConfig::default();
    config.security.csrf.enabled = false;

    TestApp::new()
        .routes(app_routes())
        .config(config)
        .with_db(db.pool())
        .build()
}

/// Pull the session cookie pair (`name=value`) out of a `Set-Cookie` response.
fn session_cookie(resp: &autumn_web::test::TestResponse) -> String {
    let set_cookie = resp
        .header("set-cookie")
        .expect("auth response should set a session cookie");
    set_cookie
        .split(';')
        .next()
        .expect("cookie has a name=value pair")
        .to_owned()
}

/// Sign up a user and return their session cookie.
async fn signup(client: &TestClient, email: &str) -> String {
    let resp = client
        .post("/signup")
        .form(&format!("email={email}&password=password123"))
        .send()
        .await;
    resp.assert_status(303);
    session_cookie(&resp)
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn signup_login_dashboard_returns_200() {
    let client = db_client().await;
    let cookie = signup(&client, "founder@acme.test").await;

    // The tenant-scoped dashboard renders for the signed-in session.
    client
        .get("/dashboard")
        .header("cookie", &cookie)
        .send()
        .await
        .assert_ok()
        .assert_body_contains("Projects");

    // Logging back in lands on the same tenant dashboard.
    let login = client
        .post("/login")
        .form("email=founder@acme.test&password=password123")
        .send()
        .await;
    login.assert_status(303);
    let cookie = session_cookie(&login);
    client
        .get("/dashboard")
        .header("cookie", &cookie)
        .send()
        .await
        .assert_ok();
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn tenants_are_isolated() {
    let client = db_client().await;

    let acme = signup(&client, "a@acme.test").await;
    let globex = signup(&client, "b@globex.test").await;

    // Acme creates a project.
    client
        .post("/dashboard/projects")
        .header("cookie", &acme)
        .form("name=Alpha")
        .send()
        .await
        .assert_status(303);

    // Acme sees its project …
    client
        .get("/dashboard")
        .header("cookie", &acme)
        .send()
        .await
        .assert_ok()
        .assert_body_contains("Alpha");

    // … but Globex (a different tenant) does not.
    let globex_view = client
        .get("/dashboard")
        .header("cookie", &globex)
        .send()
        .await;
    globex_view.assert_ok();
    assert!(
        !globex_view.text().contains("Alpha"),
        "tenant isolation breached: Globex can see Acme's project"
    );
}
