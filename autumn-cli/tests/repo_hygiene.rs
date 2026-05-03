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
