//! `autumn destroy scaffold` must not strip the `storage`/`markdown`
//! autumn-web features while hand-written code imports the module
//! (issue #2186).
//!
//! `autumn destroy scaffold` keeps an `autumn-web` feature only if the
//! remaining source contains its marker. The `storage` and `markdown`
//! markers ended in `::`. A module import (`use autumn_web::storage;`,
//! `use autumn_web::markdown as md;`) ends at the module name, so the scan
//! did not find it. `destroy` then removed the feature, and the build failed
//! in a file the generator did not write. The markers deliberately carry no
//! trailing `::` (as `csv`'s already did for #2184).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const fn autumn_bin() -> &'static str {
    env!("CARGO_BIN_EXE_autumn")
}

fn run_autumn_ok(dir: &Path, args: &[&str]) {
    let output = Command::new(autumn_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run autumn");
    assert!(
        output.status.success(),
        "autumn {args:?} failed (exit={:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// `autumn new` + `generate scaffold Post <cols>`, returning the tempdir
/// guard and the generated project root.
fn scaffold_project(name: &str, cols: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    run_autumn_ok(tmp.path(), &["new", name]);
    let project = tmp.path().join(name);
    let mut args = vec!["generate", "scaffold", "Post"];
    args.extend_from_slice(cols);
    run_autumn_ok(&project, &args);
    (tmp, project)
}

/// The `autumn-web = { … }` dependency line of the project's Cargo.toml.
/// The assertions read ONLY this line, so other text in the manifest can
/// neither pass nor fail them.
fn autumn_web_dep_line(project: &Path) -> String {
    let cargo = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    cargo
        .lines()
        .find(|l| l.starts_with("autumn-web = {"))
        .unwrap_or_else(|| panic!("no autumn-web dependency line:\n{cargo}"))
        .to_owned()
}

// ── `storage` ─────────────────────────────────────────────────────────────
// An attachment column enables autumn-web's `storage` (+ `multipart`)
// features. The hand-written file goes directly in `src/` — NOT in the
// feature's `owner_dir` (`src/models`), where the sibling check would keep
// the feature anyway and the test could not fail.

#[test]
fn destroy_keeps_the_storage_feature_for_a_module_import() {
    // `use autumn_web::storage;` ends at the module name — there is no
    // trailing `::` for a separator-requiring marker to find.
    let (_tmp, project) =
        scaffold_project("storage-destroy-mod", &["title:String", "cover:attachment"]);
    fs::write(
        project.join("src/handwritten.rs"),
        "use autumn_web::storage;\n\npub fn maybe_blob() -> Option<storage::Blob> {\n    None\n}\n",
    )
    .unwrap();
    run_autumn_ok(&project, &["destroy", "scaffold", "Post", "--force"]);
    let dep_line = autumn_web_dep_line(&project);
    assert!(
        dep_line.contains("\"storage\""),
        "a module-level `use autumn_web::storage;` still needs the feature:\n{dep_line}"
    );
}

#[test]
fn destroy_keeps_the_storage_feature_for_a_renamed_module_import() {
    // `use … as store;` ends at the rename, not at `::` either.
    let (_tmp, project) = scaffold_project(
        "storage-destroy-alias",
        &["title:String", "cover:attachment"],
    );
    fs::write(
        project.join("src/handwritten.rs"),
        "use autumn_web::storage as store;\n\npub fn maybe_blob() -> Option<store::Blob> {\n    None\n}\n",
    )
    .unwrap();
    run_autumn_ok(&project, &["destroy", "scaffold", "Post", "--force"]);
    let dep_line = autumn_web_dep_line(&project);
    assert!(
        dep_line.contains("\"storage\""),
        "a renamed module import still needs the feature:\n{dep_line}"
    );
}

#[test]
fn destroy_removes_the_storage_feature_when_nothing_references_it() {
    let (_tmp, project) = scaffold_project(
        "storage-destroy-gone",
        &["title:String", "cover:attachment"],
    );
    run_autumn_ok(&project, &["destroy", "scaffold", "Post", "--force"]);
    let dep_line = autumn_web_dep_line(&project);
    assert!(
        !dep_line.contains("\"storage\""),
        "with no hand-written use, destroy must remove the feature it added:\n{dep_line}"
    );
}

// ── `markdown` ────────────────────────────────────────────────────────────
// A `richtext` column enables autumn-web's `markdown` feature. The
// hand-written file goes directly in `src/` — NOT in the feature's
// `owner_dir` (`src/routes`), where the sibling check would keep the
// feature anyway and the test could not fail.

#[test]
fn destroy_keeps_the_markdown_feature_for_a_module_import() {
    let (_tmp, project) =
        scaffold_project("markdown-destroy-mod", &["title:String", "body:richtext"]);
    fs::write(
        project.join("src/handwritten.rs"),
        "use autumn_web::markdown;\n\npub fn render_it(src: &str) -> String {\n    markdown::render(src)\n}\n",
    )
    .unwrap();
    run_autumn_ok(&project, &["destroy", "scaffold", "Post", "--force"]);
    let dep_line = autumn_web_dep_line(&project);
    assert!(
        dep_line.contains("\"markdown\""),
        "a module-level `use autumn_web::markdown;` still needs the feature:\n{dep_line}"
    );
}

#[test]
fn destroy_keeps_the_markdown_feature_for_a_renamed_module_import() {
    let (_tmp, project) =
        scaffold_project("markdown-destroy-alias", &["title:String", "body:richtext"]);
    fs::write(
        project.join("src/handwritten.rs"),
        "use autumn_web::markdown as md;\n\npub fn render_it(src: &str) -> String {\n    md::render(src)\n}\n",
    )
    .unwrap();
    run_autumn_ok(&project, &["destroy", "scaffold", "Post", "--force"]);
    let dep_line = autumn_web_dep_line(&project);
    assert!(
        dep_line.contains("\"markdown\""),
        "a renamed module import still needs the feature:\n{dep_line}"
    );
}

#[test]
fn destroy_removes_the_markdown_feature_when_nothing_references_it() {
    let (_tmp, project) =
        scaffold_project("markdown-destroy-gone", &["title:String", "body:richtext"]);
    run_autumn_ok(&project, &["destroy", "scaffold", "Post", "--force"]);
    let dep_line = autumn_web_dep_line(&project);
    assert!(
        !dep_line.contains("\"markdown\""),
        "with no hand-written use, destroy must remove the feature it added:\n{dep_line}"
    );
}
