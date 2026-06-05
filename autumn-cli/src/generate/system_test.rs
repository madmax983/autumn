//! `autumn generate system-test` — emit a system-test skeleton.
//!
//! Creates:
//!   - `tests/system/<snake>.rs` — test file wired to the `system-tests` feature
//!   - `Cargo.toml` — adds `[features] system-tests` and `[[test]]` entry if absent
//!
//! # Usage
//!
//!   autumn generate system-test `TodoFlow`
//!   autumn generate system-test `TodoFlow` --dry-run

use std::fmt::Write as _;
use std::path::Path;

use super::emit::Plan;
use super::model::validate_resource_name;
use super::naming::{pascal, snake};
use super::{Flags, GenerateError, ensure_project_root};

/// Returns `true` if `trimmed` is a TOML section header matching `name`,
/// with or without a trailing inline comment (e.g. `[workspace] # root`).
fn is_toml_header(trimmed: &str, name: &str) -> bool {
    let pat = format!("[{name}]");
    trimmed == pat
        || trimmed
            .strip_prefix(pat.as_str())
            .is_some_and(|rest| rest.trim_start().starts_with('#'))
}

fn is_virtual_workspace(cargo_toml: &str) -> bool {
    let mut has_workspace = false;
    let mut has_package = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if is_toml_header(trimmed, "workspace") {
            has_workspace = true;
        } else if is_toml_header(trimmed, "package") {
            has_package = true;
        }
    }
    has_workspace && !has_package
}

/// Compute the file actions for `autumn generate system-test`.
///
/// # Errors
/// Project layout and name validation errors surface here.
pub fn plan_system_test(project_root: &Path, name: &str) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;
    validate_resource_name(name)?;

    let snake_name = snake(name);
    let pascal_name = pascal(name);
    let mut plan = Plan::new(project_root);

    // Ensure tests/system/ directory exists by placing the file there.
    plan.create(
        project_root
            .join("tests")
            .join("system")
            .join(format!("{snake_name}.rs")),
        render_system_test_file(&snake_name, &pascal_name),
    );

    // Patch Cargo.toml: add system-tests feature + [[test]] entry.
    let cargo_path = project_root.join("Cargo.toml");
    let existing = std::fs::read_to_string(&cargo_path).map_err(GenerateError::Io)?;

    // Reject virtual workspace manifests (they have [workspace] but no
    // [package]). Patching a virtual manifest with [features] or [[test]]
    // would corrupt it; the user should run this command inside a package.
    if is_virtual_workspace(&existing) {
        return Err(GenerateError::Config(
            "Cargo.toml is a virtual workspace manifest (no [package] section). \
             Run `autumn generate system-test` from inside a package directory."
                .to_owned(),
        ));
    }
    let patched = patch_cargo_toml(&existing, &snake_name);
    if patched != existing {
        plan.modify(cargo_path, patched);
    }

    Ok(plan)
}

/// Returns `true` if `trimmed` is a TOML array-of-tables header `[[name]]`,
/// with or without a trailing inline comment.
fn is_array_table_header(trimmed: &str, name: &str) -> bool {
    let pat = format!("[[{name}]]");
    trimmed == pat
        || trimmed
            .strip_prefix(pat.as_str())
            .is_some_and(|rest| rest.trim_start().starts_with('#'))
}

/// Returns `true` if `trimmed` is a `[features]` table header, with or without
/// a trailing inline comment (e.g. `[features] # project features`).
fn is_features_header(trimmed: &str) -> bool {
    trimmed == "[features]"
        || trimmed
            .strip_prefix("[features]")
            .is_some_and(|rest| rest.trim_start().starts_with('#'))
}

/// Returns `true` if the `[features]` table in `cargo_toml` already contains
/// a key named `key` (i.e. the key appears within the `[features]` section,
/// not merely anywhere in the file).
fn features_section_has_key(cargo_toml: &str, key: &str) -> bool {
    let mut in_features = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if is_features_header(trimmed) {
            in_features = true;
            continue;
        }
        if in_features {
            // Another table header ends the [features] section.
            if trimmed.starts_with('[') {
                break;
            }
            // Check if this line declares the key (bare or TOML-quoted form).
            let quoted_key = format!("\"{key}\"");
            let bare_match = trimmed
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='));
            let quoted_match = trimmed
                .strip_prefix(quoted_key.as_str())
                .is_some_and(|rest| rest.trim_start().starts_with('='));
            if bare_match || quoted_match {
                return true;
            }
        }
    }
    false
}

/// Returns `true` if any `[[test]]` section in `cargo_toml` has `name = test_name`.
///
/// Scans section-by-section so key order and whitespace within the section
/// don't cause false negatives.
fn test_section_names_test(cargo_toml: &str, test_name: &str) -> bool {
    let expected = format!("\"{test_name}\"");
    let mut in_test = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if is_array_table_header(trimmed, "test") {
            in_test = true;
            continue;
        }
        if in_test {
            if trimmed.starts_with('[') {
                // This header ends the current [[test]] section. If it's
                // another [[test]], re-enter immediately so we don't skip it.
                in_test = is_array_table_header(trimmed, "test");
                continue;
            }
            if let Some(after) = trimmed.strip_prefix("name") {
                let after = after.trim_start();
                if let Some(val) = after.strip_prefix('=') {
                    // Strip any trailing TOML inline comment before comparing.
                    let val = val.trim();
                    let val = val.split_once(" #").map_or(val, |(v, _)| v.trim());
                    if val == expected {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Returns the byte offset immediately after the `[features]` header line
/// (i.e. after the newline that terminates the header), handling both LF and
/// CRLF line endings and any inline comments on the header line.
fn find_features_header_end(cargo_toml: &str) -> Option<usize> {
    let mut pos = 0;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        let is_header = is_features_header(trimmed);
        // Advance pos past this line (include its line ending).
        pos += line.len();
        // Account for \r\n vs \n.
        if cargo_toml[pos..].starts_with("\r\n") {
            pos += 2;
        } else if cargo_toml[pos..].starts_with('\n') {
            pos += 1;
        }
        if is_header {
            return Some(pos);
        }
    }
    None
}

/// Patch `Cargo.toml` content to add the `system-tests` feature (under
/// `[features]` only, not in `[dependencies]`) and a `[[test]]` entry for this
/// test file if they are not already present.
fn patch_cargo_toml(existing: &str, snake_name: &str) -> String {
    let mut out = existing.to_owned();

    // 1. Add [features] system-tests entry if not already in the [features] table.
    // We scan only the [features] section so that a dev-dependency enabling
    // autumn-web/system-tests does not suppress the local feature definition
    // (which is required by `--features system-tests` and `#[cfg(feature = ...)]`).
    let feature_line = "system-tests = [\"autumn-web/system-tests\"]";
    if !features_section_has_key(&out, "system-tests") {
        // Find the byte offset of the end of the "[features]" header line so we
        // can insert immediately after it regardless of line ending style (LF or
        // CRLF) or trailing inline comments on the header.
        if let Some(insert_pos) = find_features_header_end(&out) {
            // If the header had no trailing newline (EOF case), add one before
            // the feature line so the manifest stays valid TOML.
            let prefix = if insert_pos > 0 && !out[..insert_pos].ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.insert_str(insert_pos, &format!("{prefix}{feature_line}\n"));
        } else {
            let _ = write!(out, "\n[features]\n{feature_line}\n");
        }
    }

    // 2. Add [[test]] entry if no [[test]] section already names this test.
    // Scan section-by-section so key order and whitespace don't matter.
    if !test_section_names_test(&out, snake_name) {
        let _ = write!(
            out,
            "\n[[test]]\nname = \"{snake_name}\"\npath = \"tests/system/{snake_name}.rs\"\n"
        );
    }

    out
}

fn render_system_test_file(snake_name: &str, pascal_name: &str) -> String {
    format!(
        r#"//! System test: {pascal_name}
//!
//! Generated by `autumn generate system-test {pascal_name}`.
//!
//! Run:
//!   cargo test --features system-tests --test {snake_name} -- --include-ignored
//!
//! Requires Chromium:
//!   apt-get install chromium-browser          # Ubuntu/Debian
//!   brew install --cask chromium              # macOS
//!   AUTUMN_CHROMIUM=/path/to/chrome cargo test # custom binary

#![cfg(feature = "system-tests")]

use autumn_web::prelude::*;
use autumn_web::system_test::SystemTest;

// ── Route handlers under test ──────────────────────────────────────────────

#[get("/")]
async fn index() -> String {{
    format!(
        "<!DOCTYPE html><html><head><title>{pascal_name}</title></head>\
         <body><h1>{pascal_name}</h1></body></html>"
    )
}}

// ── Tests ─────────────────────────────────────────────────────────────────

/// Happy-path: visit the index page and assert on rendered content.
///
/// Requires Chromium on the host. Skipped in CI unless `AUTUMN_CHROMIUM` or
/// a system Chromium binary is available.
#[tokio::test]
#[ignore = "requires Chromium — set AUTUMN_CHROMIUM or install chromium-browser"]
async fn {snake_name}_index_renders() {{
    let runner = SystemTest::new()
        .routes(routes![index])
        .build()
        .await
        .expect("system test runner");

    let page = runner.page().await.expect("page");
    page.visit("/").await.expect("visit /");
    page.expect_text("{pascal_name}").await.expect("page title visible");
}}
"#
    )
}

/// CLI entry point.
pub fn run(name: &str, flags: Flags) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    match plan_system_test(&cwd, name).and_then(|p| p.execute(flags)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_project() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // Minimal Cargo.toml so ensure_project_root passes.
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test-project\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        tmp
    }

    #[test]
    fn plan_creates_system_test_file() {
        let tmp = temp_project();
        let plan = plan_system_test(tmp.path(), "TodoFlow").unwrap();
        plan.execute(Flags::default()).unwrap();

        let test_file = tmp.path().join("tests").join("system").join("todo_flow.rs");
        assert!(test_file.exists(), "expected {}", test_file.display());

        let content = fs::read_to_string(&test_file).unwrap();
        assert!(content.contains("TodoFlow"), "missing pascal name");
        assert!(content.contains("system-tests"), "missing feature gate");
        assert!(content.contains("#[tokio::test]"), "missing test attr");
        assert!(
            content.contains("#[ignore"),
            "test must be #[ignore] by default (requires Chromium)"
        );
    }

    #[test]
    fn plan_snake_cases_name() {
        let tmp = temp_project();
        plan_system_test(tmp.path(), "MyFeatureTest")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let test_file = tmp
            .path()
            .join("tests")
            .join("system")
            .join("my_feature_test.rs");
        assert!(test_file.exists());
    }

    #[test]
    fn plan_rejects_invalid_name() {
        let tmp = temp_project();
        assert!(plan_system_test(tmp.path(), "123-invalid").is_err());
    }

    #[test]
    fn plan_dry_run_writes_nothing() {
        let tmp = temp_project();
        plan_system_test(tmp.path(), "DryRunTest")
            .unwrap()
            .execute(Flags {
                dry_run: true,
                force: false,
            })
            .unwrap();

        let test_file = tmp
            .path()
            .join("tests")
            .join("system")
            .join("dry_run_test.rs");
        assert!(!test_file.exists(), "dry run should not write files");
    }

    #[test]
    fn plan_collides_without_force() {
        let tmp = temp_project();
        let flags = Flags::default();
        plan_system_test(tmp.path(), "Collision")
            .unwrap()
            .execute(flags)
            .unwrap();
        // Second attempt should fail.
        let result = plan_system_test(tmp.path(), "Collision")
            .unwrap()
            .execute(flags);
        assert!(result.is_err());
    }

    #[test]
    fn plan_patches_cargo_toml() {
        let tmp = temp_project();
        plan_system_test(tmp.path(), "TodoFlow")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("system-tests"),
            "Cargo.toml must define system-tests feature"
        );
        assert!(
            cargo.contains("[[test]]"),
            "Cargo.toml must have a [[test]] entry"
        );
        assert!(
            cargo.contains("todo_flow"),
            "[[test]] must reference the generated file"
        );
    }

    #[test]
    fn patch_cargo_toml_crlf_features_header() {
        // Cargo.toml with CRLF line endings should still have the feature inserted.
        let crlf = "[package]\r\nname = \"x\"\r\n\r\n[features]\r\nother = []\r\n";
        let patched = patch_cargo_toml(crlf, "my_test");
        assert!(
            patched.contains("system-tests"),
            "feature must be inserted even with CRLF line endings"
        );
        assert!(
            patched.contains("[[test]]"),
            "[[test]] entry must also be present"
        );
    }

    #[test]
    fn patch_cargo_toml_features_header_with_comment() {
        let src = "[package]\nname = \"x\"\n\n[features] # project features\nother = []\n";
        let patched = patch_cargo_toml(src, "my_test");
        assert!(
            patched.contains("system-tests"),
            "feature must be inserted after a commented header"
        );
    }

    #[test]
    fn patch_cargo_toml_idempotent_with_commented_features_header() {
        // If [features] already has system-tests under a commented header,
        // patching again must not insert a second key.
        let src = "[package]\nname = \"x\"\n\n[features] # project features\nsystem-tests = [\"autumn-web/system-tests\"]\nother = []\n";
        let patched = patch_cargo_toml(src, "my_test");
        let count = patched.matches("system-tests =").count();
        assert_eq!(
            count, 1,
            "system-tests key must appear exactly once; got {count}"
        );
    }

    #[test]
    fn find_features_header_end_no_trailing_newline() {
        // [features] as the very last line with no trailing newline — pos does
        // not advance past a newline but the header is still found.
        let src = "[package]\nname = \"x\"\n\n[features]";
        let pos = super::find_features_header_end(src);
        // Should point to the byte just past "[features]" (end of string).
        assert!(
            pos.is_some(),
            "should find header even without trailing newline"
        );
        let pos = pos.unwrap();
        assert_eq!(pos, src.len(), "pos should be at end of string");
    }

    #[test]
    fn find_features_header_end_no_features_section() {
        let src = "[package]\nname = \"x\"\n";
        assert!(
            super::find_features_header_end(src).is_none(),
            "should return None when no [features] header present"
        );
    }

    #[test]
    fn features_section_has_key_recognizes_commented_header_with_key() {
        let src = "[features] # project features\nsystem-tests = [\"autumn-web/system-tests\"]\n";
        assert!(
            super::features_section_has_key(src, "system-tests"),
            "should detect key under a commented [features] header"
        );
    }

    #[cfg(unix)]
    #[test]
    fn plan_system_test_errors_on_unreadable_cargo_toml() {
        use std::os::unix::fs::PermissionsExt;
        // Skip when running as root (CI containers) since chmod 000 is ineffective.
        if std::fs::read("/etc/shadow").is_ok() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let cargo_path = tmp.path().join("Cargo.toml");
        fs::write(
            &cargo_path,
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::set_permissions(&cargo_path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = plan_system_test(tmp.path(), "IoError");
        fs::set_permissions(&cargo_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            result.is_err(),
            "plan_system_test must propagate Cargo.toml read errors"
        );
    }

    #[test]
    fn patch_cargo_toml_no_trailing_newline_on_features_header() {
        // When [features] is the last line with no newline, the feature line
        // must be on a new line (not run together as "[features]system-tests").
        let src = "[package]\nname = \"x\"\n\n[features]";
        let patched = patch_cargo_toml(src, "eof_test");
        assert!(
            patched.contains("[features]\nsystem-tests"),
            "feature line must follow [features] on a new line; got: {patched:?}"
        );
    }

    #[test]
    fn test_section_names_test_multiple_sections() {
        // When there are multiple [[test]] sections, the scanner must check
        // all of them and not stop after the first one.
        let src = "[[test]]\nname = \"other_test\"\npath = \"tests/other.rs\"\n\
                   \n[[test]]\nname = \"my_test\"\npath = \"tests/my_test.rs\"\n";
        assert!(
            super::test_section_names_test(src, "my_test"),
            "should find my_test in the second [[test]] section"
        );
        assert!(
            !super::test_section_names_test(src, "missing"),
            "should return false for a name not in any section"
        );
    }

    #[test]
    fn plan_rejects_virtual_workspace() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\"]\n",
        )
        .unwrap();
        let result = plan_system_test(tmp.path(), "MyTest");
        assert!(
            result.is_err(),
            "should reject a virtual workspace manifest"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("virtual workspace"),
            "error should mention virtual workspace, got: {msg}"
        );
    }

    #[test]
    fn plan_rejects_virtual_workspace_with_comment() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace] # root manifest\nmembers = [\"app\"]\n",
        )
        .unwrap();
        let result = plan_system_test(tmp.path(), "MyTest");
        assert!(
            result.is_err(),
            "should reject a virtual workspace with a commented header"
        );
    }

    #[test]
    fn test_section_names_test_inline_comment() {
        // name values with trailing TOML inline comments must still be recognised.
        let src =
            "[[test]]\nname = \"todo_flow\" # browser test\npath = \"tests/system/todo_flow.rs\"\n";
        assert!(
            super::test_section_names_test(src, "todo_flow"),
            "should match name even when it has a trailing inline comment"
        );
    }

    #[test]
    fn patch_cargo_toml_idempotent_with_quoted_feature_key() {
        // A valid TOML manifest may quote the key: `"system-tests" = [...]`.
        // The generator must recognise this and not insert a duplicate bare key.
        let src = "[package]\nname = \"x\"\n\n[features]\n\"system-tests\" = [\"autumn-web/system-tests\"]\n";
        let patched = patch_cargo_toml(src, "my_test");
        // Confirm no bare (unquoted) system-tests key was inserted.
        assert!(
            !patched.contains("\nsystem-tests ="),
            "must not insert a bare system-tests key when a quoted one already exists; patched: {patched:?}"
        );
        // The feature content should not have grown — the original and patched
        // [features] section should be identical.
        let orig_lines: Vec<_> = src.lines().filter(|l| l.contains("system-tests")).collect();
        let patch_lines: Vec<_> = patched
            .lines()
            .filter(|l| l.contains("system-tests"))
            .collect();
        assert_eq!(
            orig_lines, patch_lines,
            "patching must not change the existing system-tests feature line"
        );
    }

    #[test]
    fn plan_force_overwrites() {
        let tmp = temp_project();
        plan_system_test(tmp.path(), "Force")
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        // Second attempt with force should succeed.
        plan_system_test(tmp.path(), "Force")
            .unwrap()
            .execute(Flags {
                dry_run: false,
                force: true,
            })
            .unwrap();
    }
}
