//! End-to-end integration tests for `autumn generate`.
//!
//! These run the real `autumn` binary against a freshly-`new`-ed project and
//! assert the produced filesystem matches the documented contract — covering
//! the user-facing flow described in [Issue #493].
//!
//! [Issue #493]: https://github.com/madmax983/autumn/issues/493

use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Spawn the production `autumn` binary in `dir` with the given args and
/// assert it exits successfully, returning the captured stdout + stderr.
fn run_autumn(dir: &Path, args: &[&str]) -> (String, String) {
    let autumn_bin = env!("CARGO_BIN_EXE_autumn");
    let output = Command::new(autumn_bin)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run autumn");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "autumn {args:?} failed (exit={:?})\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code(),
    );
    (stdout, stderr)
}

/// Spawn the production `autumn` binary with environment overrides.
fn run_autumn_with_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> (String, String) {
    let autumn_bin = env!("CARGO_BIN_EXE_autumn");
    let output = Command::new(autumn_bin)
        .args(args)
        .current_dir(dir)
        .envs(envs.iter().copied())
        .output()
        .expect("failed to run autumn");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "autumn {args:?} failed (exit={:?})\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code(),
    );
    (stdout, stderr)
}

/// Same as [`run_autumn`] but expects a non-zero exit code.
fn run_autumn_failing(dir: &Path, args: &[&str]) -> (String, String, Option<i32>) {
    let autumn_bin = env!("CARGO_BIN_EXE_autumn");
    let output = Command::new(autumn_bin)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run autumn");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (stdout, stderr, output.status.code())
}

/// `autumn new <name>` in a fresh tempdir, returning that tempdir + the
/// project root inside it.
fn fresh_project(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    run_autumn(tmp.path(), &["new", name]);
    let project = tmp.path().join(name);
    (tmp, project)
}

fn patch_generated_cargo_toml(project_dir: &Path) {
    let cargo_toml_path = project_dir.join("Cargo.toml");
    let mut content = fs::read_to_string(&cargo_toml_path).unwrap();
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let autumn_web = workspace_root.join("autumn");
    write!(
        content,
        "\n[patch.crates-io]\nautumn-web = {{ path = \"{}\" }}\n",
        autumn_web.display().to_string().replace('\\', "/")
    )
    .unwrap();
    fs::write(&cargo_toml_path, content).unwrap();
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child_output(child: &mut Child) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    (stdout, stderr)
}

// Async HTTP poll used by tests that already run inside a Tokio runtime
// (e.g. #[tokio::test] tests that also drive async testcontainers).
// Using reqwest::blocking inside an existing Tokio runtime panics when the
// blocking client's internal runtime is dropped, so these tests use the
// native async reqwest::Client instead.
async fn wait_for_server_ready_async(
    mut child: Child,
    client: &reqwest::Client,
    base: &str,
) -> ServerGuard {
    for _ in 0..60 {
        if let Some(status) = child.try_wait().expect("server status") {
            let (stdout, stderr) = child_output(&mut child);
            panic!(
                "server exited before becoming ready: {status}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            );
        }

        if client.get(format!("{base}/health")).send().await.is_ok() {
            return ServerGuard(child);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let _ = child.kill();
    let _ = child.wait();
    let (stdout, stderr) = child_output(&mut child);
    panic!("server failed to become ready within 30 seconds\nstdout:\n{stdout}\nstderr:\n{stderr}");
}

#[test]
fn generate_model_in_fresh_project() {
    let (_tmp, project) = fresh_project("model-app");
    run_autumn(
        &project,
        &[
            "generate",
            "model",
            "Post",
            "title:String",
            "body:Text",
            "published:bool",
        ],
    );

    let model = fs::read_to_string(project.join("src/models/post.rs")).unwrap();
    assert!(model.contains("#[autumn_web::model]"));
    assert!(model.contains("pub struct Post"));
    assert!(model.contains("pub title: String,"));
    assert!(model.contains("pub body: String,"));
    assert!(model.contains("pub published: bool,"));

    let mod_rs = fs::read_to_string(project.join("src/models/mod.rs")).unwrap();
    assert!(mod_rs.contains("pub mod post;"));

    let schema = fs::read_to_string(project.join("src/schema.rs")).unwrap();
    assert!(schema.contains("posts (id)"));
    assert!(schema.contains("title -> Text,"));

    // The migration directory exists with both up.sql and down.sql.
    let migrations = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with("_create_posts"))
        .collect::<Vec<_>>();
    assert_eq!(migrations.len(), 1);
    let dir = migrations[0].path();
    let up = fs::read_to_string(dir.join("up.sql")).unwrap();
    assert!(up.contains("CREATE TABLE posts"));
    assert!(up.contains("title TEXT NOT NULL"));
    assert!(up.contains("published BOOLEAN NOT NULL"));
    assert!(up.contains("id BIGSERIAL PRIMARY KEY"));
    let down = fs::read_to_string(dir.join("down.sql")).unwrap();
    assert!(down.contains("DROP TABLE posts"));
}

#[test]
fn generate_model_dry_run_writes_nothing() {
    let (_tmp, project) = fresh_project("dryrun-app");
    let (stdout, _stderr) = run_autumn(
        &project,
        &["generate", "model", "Post", "title:String", "--dry-run"],
    );
    assert!(stdout.contains("Dry run"));
    assert!(stdout.contains("src/models/post.rs"));
    assert!(!project.join("src/models/post.rs").exists());
    assert!(!project.join("src/schema.rs").exists());
}

#[test]
fn generate_model_collision_without_force() {
    let (_tmp, project) = fresh_project("collide-app");
    run_autumn(&project, &["generate", "model", "Post", "title:String"]);
    // Re-run without --force. Should fail with collision message.
    let (_, stderr, code) =
        run_autumn_failing(&project, &["generate", "model", "Post", "title:String"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("would overwrite") && stderr.contains("post.rs"),
        "expected collision message; got stderr: {stderr}"
    );
}

#[test]
fn generate_model_force_overwrites() {
    let (_tmp, project) = fresh_project("force-app");
    run_autumn(&project, &["generate", "model", "Post", "title:String"]);
    // Modify the model file so we can detect the overwrite.
    let model_path = project.join("src/models/post.rs");
    let original = fs::read_to_string(&model_path).unwrap();
    fs::write(&model_path, "// touched").unwrap();
    run_autumn(
        &project,
        &["generate", "model", "Post", "title:String", "--force"],
    );
    let regenerated = fs::read_to_string(&model_path).unwrap();
    assert_eq!(regenerated, original);
}

#[test]
fn generate_model_invalid_field_lists_supported_set() {
    let (_tmp, project) = fresh_project("badtype-app");
    let (_, stderr, code) =
        run_autumn_failing(&project, &["generate", "model", "Post", "price:Decimal"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("unsupported type"));
    assert!(stderr.contains("Supported:"));
    assert!(stderr.contains("String"));
}

#[test]
fn generate_migration_add_columns_emits_alter() {
    let (_tmp, project) = fresh_project("migrate-app");
    run_autumn(
        &project,
        &["generate", "migration", "AddTitleToPosts", "title:String"],
    );
    let migrations = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with("_add_title_to_posts")
        })
        .collect::<Vec<_>>();
    assert_eq!(migrations.len(), 1);
    let up = fs::read_to_string(migrations[0].path().join("up.sql")).unwrap();
    assert!(up.contains("ALTER TABLE posts ADD COLUMN title TEXT NOT NULL"));
}

#[test]
fn generate_migration_unknown_pattern_is_empty() {
    let (_tmp, project) = fresh_project("empty-mig-app");
    run_autumn(&project, &["generate", "migration", "BackfillSomething"]);
    let migrations = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with("_backfill_something")
        })
        .collect::<Vec<_>>();
    assert_eq!(migrations.len(), 1);
    let up = fs::read_to_string(migrations[0].path().join("up.sql")).unwrap();
    assert!(up.is_empty());
}

#[test]
fn generate_task_emits_task_module() {
    let (_tmp, project) = fresh_project("task-app");
    run_autumn(&project, &["generate", "task", "cleanup_users"]);

    let task = fs::read_to_string(project.join("tasks/cleanup_users.rs")).unwrap();
    assert!(task.contains("#[autumn_web::task]"));
    assert!(task.contains("pub async fn cleanup_users"));
    assert!(task.contains("TaskArgs<CleanupUsersArgs>"));
    assert!(task.contains("AutumnResult<()>"));
}

#[test]
fn generate_scaffold_full_e2e_post() {
    let (_tmp, project) = fresh_project("scaffold-app");
    run_autumn(
        &project,
        &[
            "generate",
            "scaffold",
            "Post",
            "title:String",
            "body:Text",
            "published:bool",
            "subtitle:Option<String>",
            "views:Option<i64>",
            "published_at:Option<NaiveDateTime>",
            "token:Option<Uuid>",
        ],
    );

    // Model + migration + schema entry.
    assert!(project.join("src/models/post.rs").is_file());
    assert!(project.join("src/models/mod.rs").is_file());
    assert!(project.join("src/schema.rs").is_file());

    // Repository file.
    let repo = fs::read_to_string(project.join("src/repositories/post.rs")).unwrap();
    assert!(repo.contains("#[autumn_web::repository(Post, api = \"/api/posts\")]"));
    assert!(repo.contains("pub trait PostRepository"));

    // HTML routes — index/show/new/create/edit_form/update; delete goes
    // through the repository's auto-generated JSON REST API.
    let routes = fs::read_to_string(project.join("src/routes/posts.rs")).unwrap();
    for needle in [
        "#[get(\"/posts\")]",
        "#[get(\"/posts/{id}\")]",
        "#[get(\"/posts/new\")]",
        "#[post(\"/posts\")]",
        "#[get(\"/posts/{id}/edit\")]",
        "#[post(\"/posts/{id}/update\")]",
        "pub async fn index",
        "pub async fn show",
        "pub async fn new_form(",
        "pub async fn update",
        "use autumn_web::security::{CsrfFormField, CsrfToken};",
        "input type=\"hidden\" name=(csrf_field_name)",
        "(csrf_input(csrf.as_ref(), csrf_field.as_ref()))",
    ] {
        assert!(routes.contains(needle), "routes file missing: {needle}");
    }

    // Smoke test.
    let test = fs::read_to_string(project.join("tests/post.rs")).unwrap();
    assert!(test.contains("posts_index_returns_200_when_server_is_running"));
    assert!(test.contains("AUTUMN_TEST_BASE_URL"));
    assert!(!test.contains("AUTUMN_TEST_SESSION_COOKIE"));
    assert!(!test.contains("Cookie: {session_cookie}"));

    // `routes![]` registration.
    let main = fs::read_to_string(project.join("src/main.rs")).unwrap();
    assert!(main.contains("mod models;"));
    assert!(main.contains("mod routes;"));
    assert!(main.contains("mod schema;"));
    assert!(main.contains("mod repositories;"));
    for entry in [
        "routes::posts::index",
        "routes::posts::show",
        "routes::posts::new_form",
        "routes::posts::create",
        "routes::posts::edit_form",
        "routes::posts::update",
        "repositories::post::post_api_list",
        "repositories::post::post_api_get",
    ] {
        assert!(
            main.contains(entry),
            "main.rs missing routes![] entry: {entry}\n{main}"
        );
    }
    for entry in [
        "repositories::post::post_api_create",
        "repositories::post::post_api_update",
        "repositories::post::post_api_delete",
    ] {
        assert!(
            !main.contains(entry),
            "main.rs should not mount public scaffold write API route: {entry}\n{main}"
        );
    }
}

#[test]
fn generate_scaffold_accepts_metadata_flags() {
    let (_tmp, project) = fresh_project("scaffold-metadata-app");
    run_autumn(
        &project,
        &[
            "generate",
            "scaffold",
            "Bookmark",
            "url:String",
            "title:String",
            "tag:String",
            "alive:bool",
            "--index",
            "url",
            "--index",
            "tag",
            "--validate",
            "url=url",
            "--validate",
            "title=length:min=1,max=200",
            "--default",
            "alive=true",
            "--query",
            "find_by_tag:tag",
            "--query",
            "find_by_alive:alive",
        ],
    );

    let model = fs::read_to_string(project.join("src/models/bookmark.rs")).unwrap();
    assert!(model.contains("#[indexed]\n    #[validate(url)]\n    pub url: String,"));
    assert!(model.contains("#[validate(length(min = 1, max = 200))]\n    pub title: String,"));
    assert!(model.contains("#[indexed]\n    pub tag: String,"));
    assert!(model.contains("#[default]\n    pub alive: bool,"));

    let repo = fs::read_to_string(project.join("src/repositories/bookmark.rs")).unwrap();
    assert!(repo.contains("fn find_by_tag(tag: String) -> Vec<Bookmark>;"));
    assert!(repo.contains("fn find_by_alive(alive: bool) -> Vec<Bookmark>;"));

    let routes = fs::read_to_string(project.join("src/routes/bookmarks.rs")).unwrap();
    assert!(routes.contains("name=\"url\""));
    assert!(routes.contains("name=\"title\""));
    assert!(routes.contains("name=\"tag\""));
    assert!(!routes.contains("name=\"alive\""));
    assert!(routes.contains("bookmarks::tag.eq(form.tag.clone())"));
    assert!(!routes.contains("bookmarks::alive.eq(form.alive.clone())"));
    assert!(!routes.contains("form.alive"));

    let migration = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with("_create_bookmarks")
        })
        .expect("create_bookmarks migration should exist");
    let up = fs::read_to_string(migration.path().join("up.sql")).unwrap();
    assert!(up.contains("alive BOOLEAN NOT NULL DEFAULT TRUE"));
    assert!(up.contains("CREATE INDEX idx_bookmarks_url ON bookmarks (url);"));
    assert!(up.contains("CREATE INDEX idx_bookmarks_tag ON bookmarks (tag);"));

    let cargo_toml = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains("validator ="),
        "validation attributes need validator in Cargo.toml:\n{cargo_toml}"
    );
}

#[test]
fn generate_scaffold_rejects_query_name_field_mismatch() {
    let (_tmp, project) = fresh_project("scaffold-bad-query-app");
    let (_, stderr, code) = run_autumn_failing(
        &project,
        &[
            "generate",
            "scaffold",
            "Bookmark",
            "tag:String",
            "alive:bool",
            "--query",
            "find_by_alive:tag",
        ],
    );

    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("find_by_alive:tag") && stderr.contains("must match field 'tag'"),
        "expected query mismatch validation error; got stderr: {stderr}"
    );
}

#[test]
fn generate_scaffold_rejects_validator_field_type_mismatch() {
    let (_tmp, project) = fresh_project("scaffold-bad-validator-app");
    let (_, stderr, code) = run_autumn_failing(
        &project,
        &[
            "generate",
            "scaffold",
            "Bookmark",
            "alive:bool",
            "--validate",
            "alive=url",
        ],
    );

    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("alive=url") && stderr.contains("url validation requires String or Text"),
        "expected validator type validation error; got stderr: {stderr}"
    );
}

#[test]
fn generate_scaffold_rejects_i32_default_outside_sql_integer_range() {
    let (_tmp, project) = fresh_project("scaffold-bad-default-app");
    let (_, stderr, code) = run_autumn_failing(
        &project,
        &[
            "generate",
            "scaffold",
            "Counter",
            "count:i32",
            "--default",
            "count=9223372036854775807",
        ],
    );

    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("count=9223372036854775807")
            && stderr.contains("i32 defaults must fit the SQL INTEGER range"),
        "expected i32 default range validation error; got stderr: {stderr}"
    );
}

/// Slow live-HTTP check: scaffold a fresh project, run migrations against a
/// real Postgres testcontainer, boot the generated server, and assert the
/// generated HTML and JSON routes actually respond.
///
/// Ignored by default; requires Docker and `diesel` CLI on PATH. Run with:
/// `cargo test -p autumn-cli --test generate generated_scaffold_serves_posts_index_and_json_api -- --ignored --exact`
#[tokio::test]
#[ignore = "slow: starts Postgres, runs diesel migrations, builds and boots a generated app"]
async fn generated_scaffold_serves_posts_index_and_json_api() {
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;

    let (_tmp, project) = fresh_project("scaffold-live");
    patch_generated_cargo_toml(&project);

    run_autumn(
        &project,
        &[
            "generate",
            "scaffold",
            "Post",
            "title:String",
            "body:Text",
            "published:bool",
        ],
    );

    let postgres = Postgres::default()
        .start()
        .await
        .expect("failed to start Postgres testcontainer");
    let host = postgres.get_host().await.expect("postgres host");
    let pg_port = postgres
        .get_host_port_ipv4(5432)
        .await
        .expect("postgres port");
    let database_url = format!("postgres://postgres:postgres@{host}:{pg_port}/postgres");

    run_autumn_with_env(
        &project,
        &["migrate"],
        &[("AUTUMN_DATABASE__URL", database_url.as_str())],
    );

    let build = Command::new("cargo")
        .args(["build"])
        .current_dir(&project)
        .output()
        .expect("failed to run cargo build");
    assert!(
        build.status.success(),
        "cargo build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let port = free_port();
    let child = Command::new("cargo")
        .args(["run", "--quiet"])
        .current_dir(&project)
        .env("AUTUMN_SERVER__PORT", port.to_string())
        .env("AUTUMN_DATABASE__URL", &database_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn generated server");

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let _server = wait_for_server_ready_async(child, &client, &base).await;

    let response = client
        .get(format!("{base}/posts"))
        .send()
        .await
        .expect("GET /posts failed");
    assert_eq!(response.status(), 200, "GET /posts status");
    let html = response.text().await.expect("GET /posts body");
    assert!(
        html.contains("<h1>Posts</h1>") && html.contains("New Post"),
        "GET /posts did not render the generated index template:\n{html}",
    );

    let response = client
        .get(format!("{base}/api/posts"))
        .send()
        .await
        .expect("GET /api/posts failed");
    assert_eq!(response.status(), 200, "GET /api/posts status");
    let body = response.text().await.expect("GET /api/posts body");
    assert_eq!(body.trim(), "[]", "empty JSON index body");
}

#[test]
fn generate_outside_project_root_fails_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, stderr, code) =
        run_autumn_failing(tmp.path(), &["generate", "model", "Post", "title:String"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("not inside an Autumn project"));
}

#[test]
fn generate_help_documents_field_dsl() {
    let tmp = tempfile::tempdir().unwrap();
    let autumn_bin = env!("CARGO_BIN_EXE_autumn");
    let output = Command::new(autumn_bin)
        .args(["generate", "--help"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("model"));
    assert!(stdout.contains("migration"));
    assert!(stdout.contains("scaffold"));
}

#[test]
fn generate_model_help_shows_example() {
    let tmp = tempfile::tempdir().unwrap();
    let autumn_bin = env!("CARGO_BIN_EXE_autumn");
    let output = Command::new(autumn_bin)
        .args(["generate", "model", "--help"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("autumn generate model Post"));
    assert!(stdout.contains("--dry-run"));
    assert!(stdout.contains("--force"));
}

/// Slow end-to-end check: scaffold a fresh project, run `autumn generate
/// scaffold`, and `cargo check --tests` the result against the local `autumn-web`
/// crate. Verifies the generator adds every dep its emitted code needs and
/// that the generated application and smoke test actually type-check.
///
/// Ignored by default; run with `cargo test -p autumn-cli -- --ignored`.
#[test]
#[ignore = "slow: cargo-checks a fresh project — run with `cargo test -p autumn-cli -- --ignored`"]
fn generated_scaffold_cargo_checks() {
    let (_tmp, project) = fresh_project("scaffold-build");

    // Patch Cargo.toml to point at the *local* autumn-web crate (so we don't
    // depend on crates.io having this exact version published). We do NOT
    // pre-add the diesel/maud/etc deps here — that's what the generator is
    // supposed to do automatically.
    let cargo_toml_path = project.join("Cargo.toml");
    patch_generated_cargo_toml(&project);

    run_autumn(
        &project,
        &[
            "generate",
            "scaffold",
            "Post",
            "title:String",
            "body:Text",
            "published:bool",
            "subtitle:Option<String>",
            "views:Option<i64>",
            "published_at:Option<NaiveDateTime>",
            "token:Option<Uuid>",
        ],
    );

    // The generator must have added every dep its emitted code needs.
    let cargo_toml_after = fs::read_to_string(&cargo_toml_path).unwrap();
    for dep in [
        "chrono",
        "diesel",
        "diesel-async",
        "maud",
        "serde",
        "serde_json",
        "serde_urlencoded",
        "url",
    ] {
        assert!(
            cargo_toml_after.contains(&format!("{dep} =")),
            "Cargo.toml is missing '{dep}' after `generate scaffold`"
        );
    }

    let check = Command::new("cargo")
        .args(["check", "--tests"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "cargo check on generated scaffold failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr),
    );
}

// ── autumn generate auth integration tests ────────────────────────────────────

#[allow(clippy::too_many_lines)]
#[test]
fn generate_auth_in_fresh_project_creates_expected_files() {
    let (_tmp, project) = fresh_project("auth-app");
    run_autumn(&project, &["generate", "auth", "User"]);

    // Migration directory exists with up.sql and down.sql.
    let migrations: Vec<_> = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with("_create_users"))
        .collect();
    assert_eq!(migrations.len(), 1, "expected one create_users migration");
    let mig_dir = migrations[0].path();
    let up = fs::read_to_string(mig_dir.join("up.sql")).unwrap();
    assert!(
        up.contains("CREATE TABLE users"),
        "up.sql missing CREATE TABLE"
    );
    assert!(up.contains("email"), "up.sql missing email column");
    assert!(
        up.contains("password_digest"),
        "up.sql missing password_digest"
    );
    assert!(
        up.contains("reset_token_digest"),
        "up.sql missing reset_token_digest"
    );
    assert!(up.contains("UNIQUE"), "email must be UNIQUE");
    let down = fs::read_to_string(mig_dir.join("down.sql")).unwrap();
    assert!(
        down.contains("DROP TABLE users"),
        "down.sql missing DROP TABLE"
    );

    // Model file
    let model = fs::read_to_string(project.join("src/models/user.rs")).unwrap();
    assert!(model.contains("pub struct User"), "model missing struct");
    assert!(
        model.contains("pub email: String"),
        "model missing email field"
    );
    assert!(
        model.contains("pub password_digest: String"),
        "model missing password_digest"
    );
    assert!(
        !model.contains("pub password:"),
        "raw password must not be stored"
    );

    // mod.rs declares user module
    let mod_rs = fs::read_to_string(project.join("src/models/mod.rs")).unwrap();
    assert!(
        mod_rs.contains("pub mod user;"),
        "models/mod.rs missing pub mod user"
    );

    // schema.rs entry
    let schema = fs::read_to_string(project.join("src/schema.rs")).unwrap();
    assert!(
        schema.contains("users (id)"),
        "schema.rs missing users table block"
    );
    assert!(
        schema.contains("email -> Text"),
        "schema.rs missing email column"
    );
    assert!(
        schema.contains("reset_token_digest -> Nullable<Text>"),
        "schema.rs missing nullable reset_token_digest"
    );

    // Routes file
    let routes = fs::read_to_string(project.join("src/routes/auth.rs")).unwrap();
    for handler in [
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
        assert!(
            routes.contains(handler),
            "routes/auth.rs missing: {handler}"
        );
    }
    assert!(
        routes.contains("#[secured]"),
        "account route must be protected"
    );
    assert!(
        routes.contains("session.destroy"),
        "logout must destroy session"
    );
    assert!(
        routes.contains("session.rotate_id"),
        "login must rotate session id"
    );
    assert!(
        routes.contains("State(state): State<AppState>"),
        "auth routes must receive AppState so sessions use the configured auth key"
    );
    assert!(
        routes.contains("session.insert(state.auth_session_key()"),
        "auth routes must populate the configured auth session key"
    );
    assert_eq!(
        routes.matches("session.insert(\"user_id\"").count(),
        3,
        "User auth routes should only write user_id as the generated account id key"
    );
    assert!(
        routes.contains("email.split_once('@')"),
        "signup email validation should use split_once"
    );
    assert!(
        !routes.contains("email.find('@').unwrap()"),
        "signup email validation should not search for @ repeatedly"
    );

    // routes/mod.rs
    let route_mod = fs::read_to_string(project.join("src/routes/mod.rs")).unwrap();
    assert!(
        route_mod.contains("pub mod auth;"),
        "routes/mod.rs missing pub mod auth"
    );

    // Generated tests file
    let tests = fs::read_to_string(project.join("tests/auth.rs")).unwrap();
    for flow in [
        "auth_signup_returns_200",
        "auth_login_returns_200",
        "auth_logout_redirects",
        "auth_forgot_password_returns_200",
        "auth_reset_password_returns_200",
        "auth_account_rejects_anonymous",
    ] {
        assert!(tests.contains(flow), "tests/auth.rs missing: {flow}");
    }

    // Documentation
    assert!(
        project.join("docs/guide/authentication.md").exists(),
        "docs/guide/authentication.md must be created"
    );

    // main.rs registers auth routes
    let main = fs::read_to_string(project.join("src/main.rs")).unwrap();
    for entry in [
        "routes::auth::signup_form",
        "routes::auth::login_form",
        "routes::auth::logout",
        "routes::auth::account",
        "routes::auth::forgot_password_form",
        "routes::auth::reset_password_form",
    ] {
        assert!(main.contains(entry), "main.rs missing route: {entry}");
    }
}

#[test]
fn generate_auth_dry_run_writes_nothing() {
    let (_tmp, project) = fresh_project("auth-dry-app");
    let (stdout, _) = run_autumn(&project, &["generate", "auth", "User", "--dry-run"]);
    assert!(
        stdout.contains("Dry run"),
        "expected dry-run output; got: {stdout}"
    );
    assert!(
        !project.join("src/models/user.rs").exists(),
        "dry run must not create model file"
    );
    assert!(
        !project.join("src/routes/auth.rs").exists(),
        "dry run must not create routes file"
    );
    assert!(
        !project.join("tests/auth.rs").exists(),
        "dry run must not create tests file"
    );
}

#[test]
fn generate_auth_collision_without_force_fails() {
    let (_tmp, project) = fresh_project("auth-collide-app");
    run_autumn(&project, &["generate", "auth", "User"]);
    // Re-run without --force should fail with collision error.
    let (_, stderr, code) = run_autumn_failing(&project, &["generate", "auth", "User"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("would overwrite"),
        "expected collision message; got stderr: {stderr}"
    );
}

#[test]
fn generate_auth_force_overwrites_existing_files() {
    let (_tmp, project) = fresh_project("auth-force-app");
    run_autumn(&project, &["generate", "auth", "User"]);
    let model_path = project.join("src/models/user.rs");
    let original = fs::read_to_string(&model_path).unwrap();
    fs::write(&model_path, "// touched").unwrap();
    run_autumn(&project, &["generate", "auth", "User", "--force"]);
    let regenerated = fs::read_to_string(&model_path).unwrap();
    assert_eq!(
        regenerated, original,
        "--force must restore original content"
    );
}

#[test]
fn generate_auth_help_documents_command() {
    let tmp = tempfile::tempdir().unwrap();
    let autumn_bin = env!("CARGO_BIN_EXE_autumn");
    let output = Command::new(autumn_bin)
        .args(["generate", "auth", "--help"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--dry-run"),
        "help should mention --dry-run"
    );
    assert!(stdout.contains("--force"), "help should mention --force");
    assert!(stdout.contains("--totp"), "help should mention --totp");
}

// ── autumn generate auth --totp (issue #799) ──────────────────────────────────

#[allow(clippy::too_many_lines)]
#[test]
fn generate_auth_totp_creates_expected_files() {
    let (_tmp, project) = fresh_project("auth-totp-app");
    run_autumn(&project, &["generate", "auth", "User", "--totp"]);

    // Migration: TOTP columns on users + recovery_codes table.
    let migrations: Vec<_> = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with("_create_users"))
        .collect();
    assert_eq!(migrations.len(), 1, "expected one create_users migration");
    let up = fs::read_to_string(migrations[0].path().join("up.sql")).unwrap();
    assert!(
        up.contains("totp_secret_encrypted"),
        "up.sql missing totp_secret_encrypted"
    );
    assert!(up.contains("totp_enabled"), "up.sql missing totp_enabled");
    assert!(
        up.contains("CREATE TABLE recovery_codes"),
        "up.sql missing recovery_codes table"
    );
    assert!(up.contains("code_digest"), "up.sql missing code_digest");
    assert!(up.contains("used_at"), "up.sql missing used_at");
    let down = fs::read_to_string(migrations[0].path().join("down.sql")).unwrap();
    assert!(
        down.contains("DROP TABLE recovery_codes"),
        "down.sql must drop recovery_codes"
    );

    // Model gains TOTP fields; recovery_code model exists.
    let model = fs::read_to_string(project.join("src/models/user.rs")).unwrap();
    assert!(model.contains("pub totp_secret_encrypted: Option<String>"));
    assert!(model.contains("pub totp_enabled: bool"));
    assert!(
        project.join("src/models/recovery_code.rs").exists(),
        "recovery_code model missing"
    );
    let mod_rs = fs::read_to_string(project.join("src/models/mod.rs")).unwrap();
    assert!(
        mod_rs.contains("pub mod recovery_code;"),
        "models/mod.rs missing recovery_code"
    );

    // schema.rs: totp columns + recovery_codes table.
    let schema = fs::read_to_string(project.join("src/schema.rs")).unwrap();
    assert!(schema.contains("totp_secret_encrypted -> Nullable<Text>"));
    assert!(schema.contains("totp_enabled -> Bool"));
    assert!(schema.contains("recovery_codes (id)"));

    // Routes: 2FA handlers + paths.
    let routes = fs::read_to_string(project.join("src/routes/auth.rs")).unwrap();
    for needle in [
        "pub async fn two_factor_status",
        "pub async fn two_factor_enable",
        "pub async fn two_factor_confirm",
        "pub async fn two_factor_disable",
        "pub async fn login_verify",
        "otpauth://",
        "Aes256Gcm",
        "totp_pending",
    ] {
        assert!(routes.contains(needle), "routes/auth.rs missing: {needle}");
    }

    // Generated 2FA integration tests cover the full round trip.
    let tests = fs::read_to_string(project.join("tests/auth_2fa.rs")).unwrap();
    for flow in [
        "two_factor_enroll_and_confirm",
        "login_with_totp_code",
        "login_with_recovery_code",
        "recovery_code_reuse_rejected",
        "two_factor_disable",
    ] {
        assert!(tests.contains(flow), "tests/auth_2fa.rs missing: {flow}");
    }

    // Cargo deps + docs.
    let cargo = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("totp-rs ="), "Cargo.toml missing totp-rs");
    assert!(cargo.contains("aes-gcm ="), "Cargo.toml missing aes-gcm");
    let docs = fs::read_to_string(project.join("docs/guide/authentication.md")).unwrap();
    assert!(
        docs.contains("Two-Factor Authentication"),
        "docs missing 2FA section"
    );

    // main.rs registers the new routes.
    let main = fs::read_to_string(project.join("src/main.rs")).unwrap();
    for entry in [
        "routes::auth::two_factor_enable",
        "routes::auth::login_verify",
    ] {
        assert!(main.contains(entry), "main.rs missing route: {entry}");
    }
}

#[test]
fn generate_auth_without_totp_has_no_totp_artifacts() {
    let (_tmp, project) = fresh_project("auth-no-totp-app");
    run_autumn(&project, &["generate", "auth", "User"]);
    let model = fs::read_to_string(project.join("src/models/user.rs")).unwrap();
    assert!(
        !model.contains("totp_enabled"),
        "default auth must not include totp fields"
    );
    assert!(!project.join("src/models/recovery_code.rs").exists());
    assert!(!project.join("tests/auth_2fa.rs").exists());
}

/// Slow: scaffold `generate auth --totp` and `cargo check --tests` the result
/// against the local `autumn-web` crate, proving the generated 2FA app and its
/// test suite type-check with zero edits (issue #799 success metric).
#[test]
#[ignore = "slow: cargo-checks a fresh project — run with `cargo test -p autumn-cli -- --ignored`"]
fn generated_auth_totp_cargo_checks() {
    let (_tmp, project) = fresh_project("auth-totp-build");
    patch_generated_cargo_toml(&project);

    run_autumn(&project, &["generate", "auth", "User", "--totp"]);

    let cargo_after = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    for dep in ["totp-rs", "aes-gcm", "base64", "diesel", "maud", "chrono"] {
        assert!(
            cargo_after.contains(&format!("{dep} =")),
            "Cargo.toml missing '{dep}' after `generate auth --totp`"
        );
    }

    let check = Command::new("cargo")
        .args(["check", "--tests"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "cargo check on generated --totp auth failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr),
    );
}

// ── TOML config (issue #669) ──────────────────────────────────────────────────

/// Scaffold a resource using only a `--config` file, no inline CLI metadata.
#[test]
fn generate_scaffold_from_config_file() {
    let (_tmp, project) = fresh_project("scaffold-config-app");

    fs::write(
        project.join("autumn.generate.toml"),
        "[scaffold.Bookmark]\n\
         fields      = [\"url:String\", \"title:String\", \"tag:String\", \"alive:bool\"]\n\
         indexes     = [\"url\", \"tag\"]\n\
         validations = [\"url=url\", \"title=length:min=1,max=200\"]\n\
         defaults    = [\"alive=true\"]\n\
         queries     = [\"find_by_tag:tag\", \"find_by_alive:alive\"]\n",
    )
    .unwrap();

    run_autumn(
        &project,
        &[
            "generate",
            "scaffold",
            "Bookmark",
            "--config",
            "autumn.generate.toml",
        ],
    );

    let model = fs::read_to_string(project.join("src/models/bookmark.rs")).unwrap();
    assert!(
        model.contains("#[indexed]\n    #[validate(url)]\n    pub url: String,"),
        "model missing indexed+validated url field:\n{model}"
    );
    assert!(
        model.contains("#[validate(length(min = 1, max = 200))]\n    pub title: String,"),
        "model missing length-validated title field:\n{model}"
    );
    assert!(
        model.contains("#[indexed]\n    pub tag: String,"),
        "model missing indexed tag field:\n{model}"
    );
    assert!(
        model.contains("#[default]\n    pub alive: bool,"),
        "model missing defaulted alive field:\n{model}"
    );

    let repo = fs::read_to_string(project.join("src/repositories/bookmark.rs")).unwrap();
    assert!(
        repo.contains("fn find_by_tag(tag: String) -> Vec<Bookmark>;"),
        "repo missing find_by_tag query:\n{repo}"
    );
    assert!(
        repo.contains("fn find_by_alive(alive: bool) -> Vec<Bookmark>;"),
        "repo missing find_by_alive query:\n{repo}"
    );

    let migration = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with("_create_bookmarks")
        })
        .expect("create_bookmarks migration must exist");
    let up = fs::read_to_string(migration.path().join("up.sql")).unwrap();
    assert!(
        up.contains("alive BOOLEAN NOT NULL DEFAULT TRUE"),
        "SQL missing default: {up}"
    );
    assert!(
        up.contains("CREATE INDEX idx_bookmarks_url ON bookmarks (url);"),
        "SQL missing url index: {up}"
    );
    assert!(
        up.contains("CREATE INDEX idx_bookmarks_tag ON bookmarks (tag);"),
        "SQL missing tag index: {up}"
    );

    let cargo_toml = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains("validator ="),
        "Cargo.toml missing validator dep:\n{cargo_toml}"
    );
}

/// CLI flags override the corresponding TOML values when both are present.
#[test]
fn generate_scaffold_cli_overrides_toml_config() {
    let (_tmp, project) = fresh_project("scaffold-config-override-app");

    fs::write(
        project.join("autumn.generate.toml"),
        "[scaffold.Post]\nfields  = [\"title:String\", \"body:Text\"]\nindexes = [\"title\"]\n",
    )
    .unwrap();

    run_autumn(
        &project,
        &[
            "generate",
            "scaffold",
            "Post",
            "content:String",
            "--index",
            "content",
            "--config",
            "autumn.generate.toml",
        ],
    );

    let model = fs::read_to_string(project.join("src/models/post.rs")).unwrap();
    assert!(
        model.contains("pub content: String"),
        "model must have CLI field 'content': {model}"
    );
    assert!(
        !model.contains("pub title: String"),
        "model must not have TOML field 'title': {model}"
    );
    assert!(
        !model.contains("pub body:"),
        "model must not have TOML field 'body': {model}"
    );

    let migration = fs::read_dir(project.join("migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().ends_with("_create_posts"))
        .expect("create_posts migration must exist");
    let up = fs::read_to_string(migration.path().join("up.sql")).unwrap();
    assert!(
        up.contains("CREATE INDEX idx_posts_content ON posts (content);"),
        "SQL must have CLI index on 'content': {up}"
    );
    assert!(
        !up.contains("idx_posts_title"),
        "SQL must not have TOML index on 'title': {up}"
    );
}

/// A non-existent config file must cause a non-zero exit with the filename
/// mentioned in the error output.
#[test]
fn generate_scaffold_rejects_missing_config_file() {
    let (_tmp, project) = fresh_project("scaffold-missing-config-app");

    let (_, stderr, code) = run_autumn_failing(
        &project,
        &[
            "generate",
            "scaffold",
            "Post",
            "title:String",
            "--config",
            "nonexistent.toml",
        ],
    );

    assert_eq!(code, Some(1), "expected exit code 1; got {code:?}");
    assert!(
        stderr.contains("nonexistent.toml"),
        "error must mention the missing file name; got:\n{stderr}"
    );
}

/// When the config file exists but has no entry for the requested resource,
/// the command must fail with a helpful error message.
#[test]
fn generate_scaffold_rejects_config_missing_resource_section() {
    let (_tmp, project) = fresh_project("scaffold-missing-section-app");

    fs::write(
        project.join("autumn.generate.toml"),
        "[scaffold.OtherResource]\nfields = [\"name:String\"]\n",
    )
    .unwrap();

    let (_, stderr, code) = run_autumn_failing(
        &project,
        &[
            "generate",
            "scaffold",
            "Post",
            "--config",
            "autumn.generate.toml",
        ],
    );

    assert_eq!(code, Some(1), "expected exit code 1; got {code:?}");
    assert!(
        stderr.contains("Post"),
        "error must mention the resource name; got:\n{stderr}"
    );
    assert!(
        stderr.contains("autumn.generate.toml"),
        "error must mention the config file name; got:\n{stderr}"
    );
}

/// Slow compile-check: scaffold a fresh project from a TOML config file and
/// verify that `cargo check --tests` succeeds against the local `autumn-web`
/// crate.  Ensures that the config-driven generator adds every dependency its
/// emitted code needs (validator, maud, etc.) and that all generated files
/// type-check without manual edits.
///
/// Ignored by default; run with:
/// `cargo test -p autumn-cli --test generate generated_scaffold_config_cargo_checks -- --ignored --exact`
#[test]
#[ignore = "slow: cargo-checks a fresh project — run with `cargo test -p autumn-cli -- --ignored`"]
fn generated_scaffold_config_cargo_checks() {
    let (_tmp, project) = fresh_project("scaffold-config-build");

    patch_generated_cargo_toml(&project);

    fs::write(
        project.join("autumn.generate.toml"),
        "[scaffold.Bookmark]\n\
         fields      = [\"url:String\", \"title:String\", \"tag:String\", \"alive:bool\"]\n\
         indexes     = [\"url\", \"tag\"]\n\
         validations = [\"url=url\", \"title=length:min=1,max=200\"]\n\
         defaults    = [\"alive=true\"]\n\
         queries     = [\"find_by_tag:tag\", \"find_by_alive:alive\"]\n",
    )
    .unwrap();

    run_autumn(
        &project,
        &[
            "generate",
            "scaffold",
            "Bookmark",
            "--config",
            "autumn.generate.toml",
        ],
    );

    let cargo_toml = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    for dep in [
        "chrono",
        "diesel",
        "diesel-async",
        "maud",
        "serde",
        "serde_urlencoded",
        "url",
        "validator",
    ] {
        assert!(
            cargo_toml.contains(&format!("{dep} =")),
            "Cargo.toml missing '{dep}' after config-driven scaffold"
        );
    }

    let check = Command::new("cargo")
        .args(["check", "--tests"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "cargo check on config-driven scaffold failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr),
    );
}

// ── Pagination scaffold tests (issue #681) ──────────────────────────────────

#[test]
fn generate_scaffold_index_uses_paginated_repo_method() {
    let (_tmp, project) = fresh_project("scaffold-paginated-app");
    run_autumn(
        &project,
        &["generate", "scaffold", "Post", "title:String", "body:Text"],
    );

    let routes = fs::read_to_string(project.join("src/routes/posts.rs")).unwrap();

    assert!(
        routes.contains("PageRequest") || routes.contains("page_req"),
        "scaffold index must use PageRequest for pagination: {routes}"
    );
    assert!(
        routes.contains("pagination_nav") || routes.contains("pagination"),
        "scaffold index must render a pagination nav partial: {routes}"
    );
    assert!(
        routes.contains(".page("),
        "scaffold index must call the repository page() method: {routes}"
    );
    assert!(
        !routes.contains(".load(&mut *db)"),
        "scaffold index must not load every row without pagination: {routes}"
    );
    // The repository trait must be imported so `repo.page()` (a trait method)
    // resolves at compile time — without it the generated code fails with E0599.
    assert!(
        routes.contains("PostRepository"),
        "scaffold routes must import the PostRepository trait (needed to call repo.page()): {routes}"
    );
}

#[test]
fn generate_scaffold_index_uses_page_request_extractor() {
    let (_tmp, project) = fresh_project("scaffold-paginated-extractor-app");
    run_autumn(&project, &["generate", "scaffold", "Post", "title:String"]);

    let routes = fs::read_to_string(project.join("src/routes/posts.rs")).unwrap();
    // PageRequest extractor handles all clamping — no manual HashMap parsing.
    assert!(
        routes.contains("page_req: PageRequest") || routes.contains("PageRequest,"),
        "scaffold index must use the PageRequest extractor: {routes}"
    );
    assert!(
        !routes.contains("HashMap"),
        "scaffold index must not manually parse query params via HashMap: {routes}"
    );
}

#[test]
fn generate_scaffold_repository_exposes_page_method() {
    let (_tmp, project) = fresh_project("scaffold-repo-page-app");
    run_autumn(&project, &["generate", "scaffold", "Post", "title:String"]);

    let repo = fs::read_to_string(project.join("src/repositories/post.rs")).unwrap();
    // The page() and cursor_page() methods are code-generated by the
    // #[autumn_web::repository] macro — they are not written out in the source
    // file.  Asserting the macro attribute is present is the correct contract:
    // the macro tests already verify it generates page() + cursor_page().
    assert!(
        repo.contains("#[autumn_web::repository("),
        "scaffold repository must use #[autumn_web::repository] (which generates page()): {repo}"
    );
    // Sanity-check that the trait is declared (confirms the scaffold structure).
    assert!(
        repo.contains("pub trait PostRepository"),
        "scaffold repository must declare a public PostRepository trait: {repo}"
    );
}

// ── autumn generate mailer ────────────────────────────────────────────────────

#[test]
fn generate_mailer_creates_all_expected_files() {
    let (_tmp, project) = fresh_project("mailer-app");
    let (stdout, _stderr) = run_autumn(&project, &["generate", "mailer", "Welcome"]);

    assert!(
        stdout.contains("welcome.rs") || stdout.contains("Created"),
        "output should mention created files: {stdout}"
    );

    // Mailer source file — production code only, no preview.
    assert!(project.join("src/mailers/welcome.rs").is_file());
    let mailer = fs::read_to_string(project.join("src/mailers/welcome.rs")).unwrap();
    assert!(mailer.contains("pub struct WelcomeMailer"));
    assert!(mailer.contains("#[mailer]"));
    assert!(
        !mailer.contains("#[mailer_preview]"),
        "#[mailer_preview] must live in previews/, not the mailer file"
    );
    assert!(mailer.contains("pub fn welcome("));
    assert!(mailer.contains("deliver_later"));

    // HTML + text templates.
    assert!(project.join("templates/mailers/welcome.html").is_file());
    let html = fs::read_to_string(project.join("templates/mailers/welcome.html")).unwrap();
    assert!(html.contains("WelcomeMailer"));
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(project.join("templates/mailers/welcome.txt").is_file());
    let txt = fs::read_to_string(project.join("templates/mailers/welcome.txt")).unwrap();
    assert!(txt.contains("WelcomeMailer"));

    // Module index declares both the mailer and the previews sub-module.
    assert!(project.join("src/mailers/mod.rs").is_file());
    let mod_rs = fs::read_to_string(project.join("src/mailers/mod.rs")).unwrap();
    assert!(mod_rs.contains("pub mod welcome;"));
    assert!(mod_rs.contains("pub mod previews;"));

    // Smoke test.
    assert!(project.join("tests/welcome_mailer.rs").is_file());
    let test = fs::read_to_string(project.join("tests/welcome_mailer.rs")).unwrap();
    assert!(test.contains("WelcomeMailer"));
    assert!(test.contains("renders_both_bodies"));
    assert!(test.contains("html.contains") || test.contains("html body"));
    assert!(test.contains("text.contains") || test.contains("text body"));
}

#[test]
fn generate_mailer_creates_preview_files_and_wires_main() {
    let (_tmp, project) = fresh_project("mailer-preview-files-app");
    run_autumn(&project, &["generate", "mailer", "Welcome"]);

    // Separate preview file with #[mailer_preview].
    assert!(project.join("src/mailers/previews/welcome.rs").is_file());
    let preview = fs::read_to_string(project.join("src/mailers/previews/welcome.rs")).unwrap();
    assert!(preview.contains("#[mailer_preview]"));
    assert!(preview.contains("welcome_preview"));

    // Previews mod.rs.
    assert!(project.join("src/mailers/previews/mod.rs").is_file());
    let previews_mod = fs::read_to_string(project.join("src/mailers/previews/mod.rs")).unwrap();
    assert!(previews_mod.contains("pub mod welcome;"));

    // main.rs wiring.
    let main = fs::read_to_string(project.join("src/main.rs")).unwrap();
    assert!(main.contains("mod mailers;"));
    assert!(main.contains("mail_previews!["));
    assert!(main.contains("mailers::welcome::WelcomeMailer"));

    // Cargo.toml: mail feature enabled.
    let cargo = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("\"mail\""),
        "Cargo.toml must include the mail feature: {cargo}"
    );
}

#[test]
fn generate_mailer_dry_run_writes_nothing() {
    let (_tmp, project) = fresh_project("mailer-dry-app");
    let (stdout, _) = run_autumn(&project, &["generate", "mailer", "Welcome", "--dry-run"]);
    assert!(
        stdout.contains("Dry run"),
        "dry run must print Dry run header: {stdout}"
    );
    assert!(
        !project.join("src/mailers/welcome.rs").exists(),
        "dry run must not create the mailer file"
    );
    assert!(
        !project.join("src/mailers/previews/welcome.rs").exists(),
        "dry run must not create the preview file"
    );
    assert!(
        !project.join("templates/mailers/welcome.html").exists(),
        "dry run must not create html template"
    );
    assert!(
        !project.join("templates/mailers/welcome.txt").exists(),
        "dry run must not create txt template"
    );
}

#[test]
fn generate_mailer_collision_without_force_fails() {
    let (_tmp, project) = fresh_project("mailer-collide-app");
    run_autumn(&project, &["generate", "mailer", "Welcome"]);
    let (_, stderr, code) = run_autumn_failing(&project, &["generate", "mailer", "Welcome"]);
    assert_eq!(code, Some(1), "second run without --force must exit 1");
    assert!(
        stderr.contains("would overwrite") || stderr.contains("welcome.rs"),
        "must report collision: {stderr}"
    );
}

#[test]
fn generate_mailer_force_overwrites_existing() {
    let (_tmp, project) = fresh_project("mailer-force-app");
    run_autumn(&project, &["generate", "mailer", "Welcome"]);
    // Corrupt the mailer file so we can detect the overwrite.
    let path = project.join("src/mailers/welcome.rs");
    fs::write(&path, "// corrupted").unwrap();
    run_autumn(&project, &["generate", "mailer", "Welcome", "--force"]);
    let content = fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("WelcomeMailer"),
        "--force must regenerate the mailer file"
    );
    assert!(
        project.join("src/mailers/previews/welcome.rs").exists(),
        "--force must also create the preview file"
    );
}

#[test]
fn generate_mailer_preview_registry_wired_into_main() {
    let (_tmp, project) = fresh_project("mailer-preview-app");
    run_autumn(&project, &["generate", "mailer", "Welcome"]);

    let main = fs::read_to_string(project.join("src/main.rs")).unwrap();

    // The preview registry wiring must appear before `.run()`.
    let preview_pos = main
        .find("mail_previews![")
        .expect("mail_previews![] must be present in main.rs");
    let run_pos = main.find(".run()").expect(".run() must still be present");
    assert!(
        preview_pos < run_pos,
        "mail_previews![] must be wired before .run() in the builder chain"
    );
    assert!(
        main.contains("mailers::welcome::WelcomeMailer"),
        "preview registry must reference the generated mailer type"
    );
}
