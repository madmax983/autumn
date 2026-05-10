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

const FIRST_RUN_DOCS: &[&str] = &[
    "README.md",
    "docs/guide/getting-started.md",
    "docs/guide/docs-smoke.md",
    "docs/guide/deployment.md",
    "docs/guide/websockets.md",
    "docs/guide/tutorial/01-project-setup.md",
    "docs/guide/tutorial/12-whats-next.md",
    "docs/guide/macro-transparency.md",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("autumn-cli should live under the workspace root")
        .to_path_buf()
}

fn workspace_package_value(root_toml: &toml::Value, key: &str) -> String {
    root_toml
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get(key))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("workspace.package.{key} should be set"))
        .to_owned()
}

fn read_workspace_manifest(root: &Path) -> toml::Value {
    let root_manifest_path = root.join("Cargo.toml");
    let root_manifest = std::fs::read_to_string(&root_manifest_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", root_manifest_path.display()));
    toml::from_str(&root_manifest).expect("workspace Cargo.toml should parse as TOML")
}

fn read_docs_once(root: &Path) -> Vec<(&'static str, String)> {
    FIRST_RUN_DOCS
        .iter()
        .map(|doc| {
            let path = root.join(doc);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
            (*doc, content)
        })
        .collect()
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
fn first_run_docs_match_current_release_line() {
    let root = workspace_root();
    let root_toml = read_workspace_manifest(&root);
    let current_version = workspace_package_value(&root_toml, "version");
    let current_series = current_version
        .rsplit_once('.')
        .map_or(current_version.as_str(), |(series, _)| series);
    let rust_version = workspace_package_value(&root_toml, "rust-version");
    let current_health_json = format!(r#"{{ "status": "ok", "version": "{current_version}" }}"#);
    let docs = read_docs_once(&root);

    for (doc, content) in &docs {
        for stale in [
            "Rust 1.85",
            "Rust 1.86",
            "rustc 1.85",
            "rustc 1.86",
            "rust:1.86",
            "autumn-web = \"0.1.0\"",
            "version=\"0.1.0\"",
            "\"version\": \"0.1.0\"",
            "v0.1.0",
            "crates.io publication is not yet available",
        ] {
            assert!(
                !content.contains(stale),
                "{doc} still references stale first-run release/MSRV text: {stale}"
            );
        }

        if content.contains("cargo install --path autumn-cli") {
            assert!(
                content
                    .to_ascii_lowercase()
                    .contains("local development only"),
                "{doc} uses `cargo install --path autumn-cli` without clearly marking it as local development only",
            );
        }

        if content.contains("Rust ") {
            assert!(
                content.contains(&format!("Rust {rust_version}+")),
                "{doc} must state the workspace MSRV Rust {rust_version}+"
            );
        }

        if content.contains("cargo install autumn-cli") {
            assert!(
                content.contains(&format!(
                    "cargo install autumn-cli --version {current_version}"
                )),
                "{doc} must show the published CLI install command for autumn-cli {current_version}"
            );
        }

        if content.contains("autumn-web =") {
            assert!(
                content.contains(&format!("autumn-web = \"{current_series}\""))
                    || content.contains(&format!("autumn-web = \"{current_version}\""))
                    || content.contains(&format!("version = \"{current_series}\""))
                    || content.contains(&format!("version = \"{current_version}\"")),
                "{doc} must show the current autumn-web release line ({current_series} or {current_version})",
            );
        }

        if content.contains(r#""status": "ok""#) {
            assert!(
                content.contains(&current_health_json),
                "{doc} must show the current JSON health version {current_version}"
            );
        }
    }

    let docs_smoke = docs
        .iter()
        .find(|(doc, _)| *doc == "docs/guide/docs-smoke.md")
        .map(|(_, content)| content)
        .expect("docs smoke guide should be included in FIRST_RUN_DOCS");
    assert!(
        docs_smoke.contains("`/` returns `Welcome to smoke-app!`"),
        "docs-smoke must expect the root page generated by `autumn new smoke-app`"
    );
    assert!(
        docs_smoke.contains(&current_health_json),
        "docs-smoke must show the exact health JSON with version {current_version}"
    );
}

#[test]
fn release_checklist_includes_docs_smoke_gate() {
    let root = workspace_root();
    let checklist_path = root.join("docs/release-checklist.md");
    let checklist = std::fs::read_to_string(&checklist_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", checklist_path.display()));

    for required in [
        "docs-smoke",
        "docs/guide/docs-smoke.md",
        "autumn-web",
        "autumn-cli",
        "release blocker",
    ] {
        assert!(
            checklist.contains(required),
            "release checklist must include `{required}` as part of the first-run docs smoke gate",
        );
    }
}

#[test]
fn workspace_forbids_unsafe_code_for_all_members() {
    let root = workspace_root();
    let root_toml = read_workspace_manifest(&root);

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
fn publish_dry_run_script_uses_list_not_no_verify() {
    let root = workspace_root();
    let script_path = root.join("scripts/check-publish-dry-run.sh");
    let script = std::fs::read_to_string(&script_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", script_path.display()));

    // --list enumerates the files that would be in the archive and validates
    // the manifest without touching the registry.  --no-verify is intentionally
    // avoided because it rewrites workspace path deps to their pinned registry
    // versions and resolves them against crates.io, which causes false failures
    // for plugin crates that depend on autumn-web features not yet published.
    assert!(
        script.contains(r#"cargo package -p "$crate" --list --allow-dirty"#),
        "{} must use `cargo package --list` for manifest/file verification",
        script_path.display(),
    );
    assert!(
        !script.contains(r#"cargo package -p "$crate" --no-verify --allow-dirty"#),
        "{} must not use `--no-verify`; that triggers registry resolution and causes false failures for plugin crates",
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
