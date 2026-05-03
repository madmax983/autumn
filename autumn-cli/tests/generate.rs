//! End-to-end integration tests for `autumn generate`.
//!
//! These run the real `autumn` binary against a freshly-`new`-ed project and
//! assert the produced filesystem matches the documented contract — covering
//! the user-facing flow described in [Issue #493].
//!
//! [Issue #493]: https://github.com/madmax983/autumn/issues/493

use std::fs;
use std::path::Path;
use std::process::Command;

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
        ],
    );

    // Model + migration + schema entry.
    assert!(project.join("src/models/post.rs").is_file());
    assert!(project.join("src/models/mod.rs").is_file());
    assert!(project.join("src/schema.rs").is_file());

    // Repository file.
    let repo = fs::read_to_string(project.join("src/repositories/post.rs")).unwrap();
    assert!(repo.contains("#[autumn_web::repository(Post)]"));
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
        "pub async fn new_form() -> AutumnResult<Markup>",
        "pub async fn update",
    ] {
        assert!(routes.contains(needle), "routes file missing: {needle}");
    }

    // Smoke test.
    let test = fs::read_to_string(project.join("tests/post.rs")).unwrap();
    assert!(
        test.contains("posts_index_returns_200_with_authenticated_session_when_server_is_running")
    );
    assert!(test.contains("AUTUMN_TEST_BASE_URL"));
    assert!(test.contains("AUTUMN_TEST_SESSION_COOKIE"));
    assert!(test.contains("Cookie: {session_cookie}"));

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
    ] {
        assert!(
            main.contains(entry),
            "main.rs missing routes![] entry: {entry}\n{main}"
        );
    }
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
    use std::fmt::Write as _;
    let (_tmp, project) = fresh_project("scaffold-build");

    // Patch Cargo.toml to point at the *local* autumn-web crate (so we don't
    // depend on crates.io having this exact version published). We do NOT
    // pre-add the diesel/maud/etc deps here — that's what the generator is
    // supposed to do automatically.
    let cargo_toml_path = project.join("Cargo.toml");
    let mut content = fs::read_to_string(&cargo_toml_path).unwrap();
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let autumn_web = workspace_root.join("autumn");
    write!(
        content,
        "\n[patch.crates-io]\nautumn-web = {{ path = \"{}\" }}\n",
        autumn_web.display().to_string().replace('\\', "/")
    )
    .unwrap();
    fs::write(&cargo_toml_path, content).unwrap();
    // Drop build.rs so we don't need the Tailwind CLI installed.
    let _ = fs::remove_file(project.join("build.rs"));

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

    // The generator must have added every dep its emitted code needs.
    let cargo_toml_after = fs::read_to_string(&cargo_toml_path).unwrap();
    for dep in ["chrono", "diesel", "diesel-async", "maud", "serde"] {
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
