use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GENERATED_EXAMPLE_CSS: &[&str] = &[
    "examples/blog/static/css/autumn.css",
    "examples/bookmarks/static/css/autumn.css",
    "examples/bookmarks-distributed/static/css/autumn.css",
    "examples/reddit-clone/static/css/autumn.css",
    "examples/todo-app/static/css/autumn.css",
    "examples/wiki/static/css/autumn.css",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("autumn-cli should live under the workspace root")
        .to_path_buf()
}

fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("failed to run git")
}

fn member_manifest_forbids_unsafe_code(manifest_toml: &toml::Value) -> bool {
    let inherits_workspace_lints = manifest_toml
        .get("lints")
        .and_then(|lints| lints.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true);
    let local_unsafe_code_lint = manifest_toml
        .get("lints")
        .and_then(|lints| lints.get("rust"))
        .and_then(|rust| rust.get("unsafe_code"))
        .and_then(toml::Value::as_str);

    local_unsafe_code_lint.map_or(inherits_workspace_lints, |level| level == "forbid")
}

#[test]
fn workspace_forbids_unsafe_code_for_all_members() {
    let root = workspace_root();
    let root_manifest_path = root.join("Cargo.toml");
    let root_manifest = std::fs::read_to_string(&root_manifest_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", root_manifest_path.display()));
    let root_toml: toml::Value =
        toml::from_str(&root_manifest).expect("workspace Cargo.toml should parse as TOML");

    let unsafe_code_lint = root_toml
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(|lints| lints.get("rust"))
        .and_then(|rust| rust.get("unsafe_code"))
        .and_then(toml::Value::as_str);
    assert_eq!(
        unsafe_code_lint,
        Some("forbid"),
        "workspace root must set [workspace.lints.rust] unsafe_code = \"forbid\"",
    );

    let members = root_toml
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .expect("workspace.members should be an array");

    for member in members {
        let member = member
            .as_str()
            .expect("workspace member entries should be strings");
        let manifest_path = root.join(member).join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", manifest_path.display()));
        let manifest_toml: toml::Value = toml::from_str(&manifest).unwrap_or_else(|err| {
            panic!("{} should parse as TOML: {err}", manifest_path.display())
        });

        assert!(
            member_manifest_forbids_unsafe_code(&manifest_toml),
            "{member}/Cargo.toml must either inherit workspace lints or set \
             [lints.rust] unsafe_code = \"forbid\"",
        );
    }
}

#[test]
fn member_manifest_forbid_check_rejects_local_override_of_workspace_lints() {
    let manifest_toml: toml::Value = toml::from_str(
        r#"
        [lints]
        workspace = true

        [lints.rust]
        unsafe_code = "warn"
        "#,
    )
    .expect("manifest snippet should parse");

    assert!(!member_manifest_forbids_unsafe_code(&manifest_toml));
}

#[test]
fn member_manifest_forbid_check_allows_workspace_lints_without_override() {
    let manifest_toml: toml::Value = toml::from_str(
        r"
        [lints]
        workspace = true
        ",
    )
    .expect("manifest snippet should parse");

    assert!(member_manifest_forbids_unsafe_code(&manifest_toml));
}

#[test]
fn generated_example_css_is_ignored_and_untracked() {
    let root = workspace_root();

    let tracked = git(
        &root,
        &["ls-files", "--", "examples/*/static/css/autumn.css"],
    );
    assert!(
        tracked.status.success(),
        "git ls-files failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&tracked.stdout),
        String::from_utf8_lossy(&tracked.stderr),
    );
    let tracked_stdout = String::from_utf8_lossy(&tracked.stdout);
    assert!(
        tracked_stdout.trim().is_empty(),
        "generated example CSS must not be tracked:\n{tracked_stdout}",
    );

    let mut ignore_args = vec!["check-ignore", "--no-index", "--"];
    ignore_args.extend_from_slice(GENERATED_EXAMPLE_CSS);
    let ignored = git(&root, &ignore_args);
    assert!(
        ignored.status.success(),
        "generated example CSS must be ignored:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&ignored.stdout),
        String::from_utf8_lossy(&ignored.stderr),
    );

    let ignored_stdout = String::from_utf8_lossy(&ignored.stdout);
    for generated_path in GENERATED_EXAMPLE_CSS {
        assert!(
            ignored_stdout.lines().any(|line| line == *generated_path),
            "ignore rules did not match {generated_path}; matched:\n{ignored_stdout}",
        );
    }
}

#[test]
fn publish_dry_run_script_builds_crate_archives() {
    let root = workspace_root();
    let script_path = root.join("scripts/check-publish-dry-run.sh");
    let script = std::fs::read_to_string(&script_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", script_path.display()));

    assert!(
        script.contains(r#"cargo package -p "$crate" --no-verify --allow-dirty"#),
        "{} must run the real cargo package dry run so the .crate archive is assembled",
        script_path.display(),
    );
    assert!(
        !script.contains(r#"cargo package -p "$crate" --list --allow-dirty"#),
        "{} must not stop at `cargo package --list`; that only enumerates files",
        script_path.display(),
    );
}

#[test]
fn bookmarks_example_tracks_regenerated_scaffold_layout() {
    let root = workspace_root();
    let bookmarks = root.join("examples/bookmarks");

    for generated_path in [
        "src/models/bookmark.rs",
        "src/models/mod.rs",
        "src/repositories/bookmark.rs",
        "src/repositories/mod.rs",
        "src/routes/bookmarks.rs",
        "src/routes/mod.rs",
        "tests/bookmark.rs",
    ] {
        assert!(
            bookmarks.join(generated_path).is_file(),
            "issue #534 expects examples/bookmarks to keep the generated scaffold file: {generated_path}",
        );
    }

    for replaced_path in ["src/models.rs", "src/repositories.rs"] {
        assert!(
            !bookmarks.join(replaced_path).exists(),
            "issue #534 expects the old flat bookmarks source file to be replaced: {replaced_path}",
        );
    }
}
