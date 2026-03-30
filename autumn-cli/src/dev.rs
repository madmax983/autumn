//! `autumn dev` -- watch for file changes and rebuild/restart the server.
//!
//! Orchestrates a development workflow:
//! 1. Compile the project with `cargo build`.
//! 2. Start the application binary.
//! 3. Watch `src/`, `static/`, `autumn.toml`, and `Cargo.toml` for changes.
//! 4. On change, kill the running server, rebuild, and restart.
//!
//! Debounces rapid file changes (e.g. editor save + format) to avoid
//! unnecessary rebuilds.

use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Default debounce interval for file change events.
const DEBOUNCE_MS: u64 = 500;

/// File extensions that trigger a rebuild when changed.
const WATCH_EXTENSIONS: &[&str] = &["rs", "toml", "css", "html", "js", "sql"];

/// Top-level files that trigger a rebuild when changed.
const WATCH_FILES: &[&str] = &["autumn.toml", "Cargo.toml", "Cargo.lock"];

/// Directories to watch recursively.
const WATCH_DIRS: &[&str] = &["src", "static", "templates", "migrations"];

/// Run the dev server with file watching.
pub fn run(package: Option<&str>, show_config: bool) {
    if show_config {
        // SAFETY: called before spawning any threads; single-threaded at this point.
        unsafe { std::env::set_var("AUTUMN_SHOW_CONFIG", "1") };
    }
    eprintln!("\u{1F342} autumn dev\n");

    // Initial build
    if !cargo_build(package) {
        eprintln!("\u{2717} Initial build failed. Fix errors and save to retry.\n");
    }

    let binary = find_binary(package);
    let mut child = start_server(&binary);

    // Set up file watcher
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(DEBOUNCE_MS), tx)
        .expect("failed to create file watcher");

    let watcher = debouncer.watcher();

    // Watch relevant directories
    for dir in WATCH_DIRS {
        let path = Path::new(dir);
        if path.exists() {
            if let Err(e) = watcher.watch(path, notify::RecursiveMode::Recursive) {
                eprintln!("  Warning: could not watch {dir}/: {e}");
            }
        }
    }

    // Watch top-level config files
    for file in WATCH_FILES {
        let path = Path::new(file);
        if path.exists() {
            if let Err(e) = watcher.watch(path, notify::RecursiveMode::NonRecursive) {
                eprintln!("  Warning: could not watch {file}: {e}");
            }
        }
    }

    eprintln!("  Watching for changes... (press Ctrl+C to stop)\n");

    // Main event loop
    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                let changed = collect_relevant_changes(&events);
                if changed.is_empty() {
                    continue;
                }

                eprintln!("\n  Changed: {}", changed.join(", "));

                // Stop the running server
                stop_server(&mut child);

                // Rebuild
                if cargo_build(package) {
                    let binary = find_binary(package);
                    child = start_server(&binary);
                } else {
                    eprintln!("  \u{2717} Build failed. Waiting for changes...\n");
                    child = None;
                }
            }
            Ok(Err(error)) => {
                eprintln!("  Watch error: {error}");
            }
            Err(e) => {
                eprintln!("  Watch channel error: {e}");
                break;
            }
        }
    }

    stop_server(&mut child);
}

/// Collect display paths for all relevant file changes from a debounced batch.
///
/// Returns an empty vec if no changes are relevant.
fn collect_relevant_changes(events: &[notify_debouncer_mini::DebouncedEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| is_relevant_change(&e.path, e.kind))
        .map(|e| e.path.display().to_string())
        .collect()
}

/// Build a `cargo build` command for the given package.
fn build_cargo_command(package: Option<&str>) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if let Some(pkg) = package {
        cmd.args(["-p", pkg]);
    }
    cmd
}

/// Run `cargo build` for the given package. Returns true on success.
fn cargo_build(package: Option<&str>) -> bool {
    let mut cmd = build_cargo_command(package);

    eprintln!("  Compiling...");
    match cmd.status() {
        Ok(status) if status.success() => {
            eprintln!("  \u{2713} Build succeeded");
            true
        }
        Ok(_) => false,
        Err(e) => {
            eprintln!("  \u{2717} Failed to run cargo build: {e}");
            false
        }
    }
}

/// Start the application binary. Returns the child process handle.
fn start_server(binary: &Path) -> Option<Child> {
    eprintln!("  Starting server...\n");
    // Inherit stdio so tracing output (including --show-config) is visible.
    // Previously used Stdio::null(), but server logs are valuable during dev.
    match Command::new(binary)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => Some(child),
        Err(e) => {
            eprintln!("  \u{2717} Failed to start {}: {e}", binary.display());
            None
        }
    }
}

/// Stop the running server process gracefully.
fn stop_server(child: &mut Option<Child>) {
    if let Some(proc) = child {
        // Send SIGTERM on Unix for graceful shutdown
        #[cfg(unix)]
        {
            #[allow(clippy::cast_possible_wrap)]
            let pid = proc.id() as libc::pid_t;
            // SAFETY: `pid` is retrieved directly from `proc.id()`, representing a valid child
            // process we own. `libc::SIGTERM` is a standard, valid signal. Sending it to our
            // own child process is safe.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            // Wait briefly for graceful shutdown before forcing
            if wait_with_timeout(proc, Duration::from_secs(5)).is_err() {
                let _ = proc.kill();
                let _ = proc.wait();
            }
        }

        #[cfg(not(unix))]
        {
            let _ = proc.kill();
            let _ = proc.wait();
        }
    }
    *child = None;
}

/// Wait for a child process with a timeout. Returns Err if timed out.
#[cfg(unix)]
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<(), ()> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    return Err(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return Err(()),
        }
    }
}

/// Check if a file change event is relevant enough to trigger a rebuild.
fn is_relevant_change(path: &Path, kind: DebouncedEventKind) -> bool {
    if !matches!(kind, DebouncedEventKind::Any) {
        return false;
    }

    // Check top-level config files
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if WATCH_FILES.contains(&name) {
            return true;
        }
    }

    // Check file extension
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if WATCH_EXTENSIONS.contains(&ext) {
            // Ignore target directory and hidden files
            for component in path.components() {
                if let std::path::Component::Normal(name) = component {
                    let name = name.to_string_lossy();
                    if name == "target" || name.starts_with('.') {
                        return false;
                    }
                }
            }
            return true;
        }
    }

    false
}

/// Resolve a binary path from parsed cargo metadata JSON.
///
/// Extracted from `find_binary` for testability. Takes the parsed
/// `cargo metadata` output and returns the path to the debug binary.
fn resolve_binary_from_metadata(
    metadata: &serde_json::Value,
    package: Option<&str>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let target_dir = metadata["target_directory"]
        .as_str()
        .ok_or("missing target_directory in metadata")?;

    let packages = metadata["packages"]
        .as_array()
        .ok_or("missing packages array in metadata")?;

    let matching_packages: Vec<_> = package.map_or_else(
        || {
            packages
                .iter()
                .filter(|pkg| {
                    let manifest = pkg["manifest_path"].as_str().unwrap_or("");
                    Path::new(manifest)
                        .parent()
                        .is_some_and(|dir| dir.starts_with(cwd))
                })
                .collect()
        },
        |pkg_name| {
            packages
                .iter()
                .filter(|pkg| pkg["name"].as_str() == Some(pkg_name))
                .collect()
        },
    );

    let bin_name = matching_packages
        .iter()
        .flat_map(|pkg| {
            pkg["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| {
                    t["kind"]
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(|k| k == "bin"))
                })
                .filter_map(|t| t["name"].as_str().map(String::from))
        })
        .next()
        .ok_or_else(|| {
            package.map_or_else(
                || "no binary target found in current package".to_owned(),
                |pkg_name| format!("no binary target found in package '{pkg_name}'"),
            )
        })?;

    let mut path = PathBuf::from(target_dir);
    path.push("debug");
    path.push(&bin_name);

    if cfg!(target_os = "windows") {
        path.set_extension("exe");
    }

    Ok(path)
}

/// Locate the compiled binary using `cargo metadata`.
///
/// Always targets the debug profile since `autumn dev` is for development.
fn find_binary(package: Option<&str>) -> PathBuf {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .output()
        .expect("failed to run cargo metadata");

    if !output.status.success() {
        eprintln!("\u{2717} Failed to read cargo metadata");
        std::process::exit(1);
    }

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata");

    let cwd = std::env::current_dir().expect("current dir");

    resolve_binary_from_metadata(&metadata, package, &cwd).unwrap_or_else(|e| {
        eprintln!("\u{2717} {e}");
        if package.is_none() {
            eprintln!("  Hint: use -p <package> to specify the target package");
        }
        std::process::exit(1);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_relevant_change tests ───────────────────────────────────

    #[test]
    fn relevant_rust_file() {
        assert!(is_relevant_change(
            Path::new("src/main.rs"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn relevant_toml_config() {
        assert!(is_relevant_change(
            Path::new("autumn.toml"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn relevant_cargo_toml() {
        assert!(is_relevant_change(
            Path::new("Cargo.toml"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn relevant_css_file() {
        assert!(is_relevant_change(
            Path::new("static/css/style.css"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn relevant_html_file() {
        assert!(is_relevant_change(
            Path::new("templates/index.html"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn relevant_sql_migration() {
        assert!(is_relevant_change(
            Path::new("migrations/001_init.sql"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn relevant_js_file() {
        assert!(is_relevant_change(
            Path::new("static/js/app.js"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn relevant_nested_rust_file() {
        assert!(is_relevant_change(
            Path::new("src/routes/api/handlers.rs"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_target_directory() {
        assert!(!is_relevant_change(
            Path::new("target/debug/build/main.rs"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_hidden_files() {
        assert!(!is_relevant_change(
            Path::new(".git/config"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_hidden_directory_nested() {
        assert!(!is_relevant_change(
            Path::new("src/.hidden/module.rs"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_irrelevant_extensions() {
        assert!(!is_relevant_change(
            Path::new("src/notes.txt"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_non_any_events() {
        assert!(!is_relevant_change(
            Path::new("src/main.rs"),
            DebouncedEventKind::AnyContinuous,
        ));
    }

    #[test]
    fn cargo_lock_triggers_rebuild() {
        assert!(is_relevant_change(
            Path::new("Cargo.lock"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_file_without_extension() {
        assert!(!is_relevant_change(
            Path::new("src/Makefile"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_image_files() {
        assert!(!is_relevant_change(
            Path::new("static/logo.png"),
            DebouncedEventKind::Any,
        ));
    }

    #[test]
    fn ignores_target_nested_deeply() {
        assert!(!is_relevant_change(
            Path::new("target/release/deps/libfoo.rs"),
            DebouncedEventKind::Any,
        ));
    }

    // ── collect_relevant_changes tests ─────────────────────────────

    #[test]
    fn collect_changes_filters_irrelevant() {
        let events = vec![
            notify_debouncer_mini::DebouncedEvent {
                path: PathBuf::from("src/main.rs"),
                kind: DebouncedEventKind::Any,
            },
            notify_debouncer_mini::DebouncedEvent {
                path: PathBuf::from("README.md"),
                kind: DebouncedEventKind::Any,
            },
            notify_debouncer_mini::DebouncedEvent {
                path: PathBuf::from("src/lib.rs"),
                kind: DebouncedEventKind::Any,
            },
        ];
        let changed = collect_relevant_changes(&events);
        assert_eq!(changed.len(), 2);
        assert!(changed.iter().any(|c| c.contains("main.rs")));
        assert!(changed.iter().any(|c| c.contains("lib.rs")));
    }

    #[test]
    fn collect_changes_returns_empty_for_no_relevant() {
        let events = vec![
            notify_debouncer_mini::DebouncedEvent {
                path: PathBuf::from("README.md"),
                kind: DebouncedEventKind::Any,
            },
            notify_debouncer_mini::DebouncedEvent {
                path: PathBuf::from("target/debug/app"),
                kind: DebouncedEventKind::Any,
            },
        ];
        let changed = collect_relevant_changes(&events);
        assert!(changed.is_empty());
    }

    #[test]
    fn collect_changes_handles_empty_events() {
        let changed = collect_relevant_changes(&[]);
        assert!(changed.is_empty());
    }

    // ── build_cargo_command tests ──────────────────────────────────

    #[test]
    fn build_command_without_package() {
        let cmd = build_cargo_command(None);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(cmd.get_program(), "cargo");
        assert_eq!(args, &["build"]);
    }

    #[test]
    fn build_command_with_package() {
        let cmd = build_cargo_command(Some("my-app"));
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(cmd.get_program(), "cargo");
        assert_eq!(args, &["build", "-p", "my-app"]);
    }

    // ── start_server tests ─────────────────────────────────────────

    #[test]
    fn start_server_returns_none_for_missing_binary() {
        let result = start_server(Path::new("/nonexistent/binary/path"));
        assert!(result.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn start_server_returns_child_for_valid_binary() {
        let child = start_server(Path::new("/bin/sleep"));
        assert!(child.is_some());
        // Clean up
        let mut child = child.unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }

    // ── resolve_binary_from_metadata tests ─────────────────────────

    /// Build the expected binary path, accounting for `.exe` on Windows.
    fn expected_binary(path: &str) -> PathBuf {
        let mut p = PathBuf::from(path);
        if cfg!(target_os = "windows") {
            p.set_extension("exe");
        }
        p
    }

    fn sample_metadata(target_dir: &str, pkg_name: &str, manifest_dir: &str) -> serde_json::Value {
        serde_json::json!({
            "target_directory": target_dir,
            "packages": [{
                "name": pkg_name,
                "manifest_path": format!("{manifest_dir}/Cargo.toml"),
                "targets": [{
                    "name": pkg_name,
                    "kind": ["bin"],
                    "src_path": format!("{manifest_dir}/src/main.rs")
                }]
            }]
        })
    }

    #[test]
    fn resolve_binary_by_package_name() {
        let metadata = sample_metadata("/tmp/target", "hello", "/projects/hello");
        let result =
            resolve_binary_from_metadata(&metadata, Some("hello"), Path::new("/projects/hello"));
        assert!(result.is_ok());
        let path = result.unwrap();
        assert_eq!(path, expected_binary("/tmp/target/debug/hello"));
    }

    #[test]
    fn resolve_binary_by_cwd() {
        let metadata = sample_metadata("/tmp/target", "hello", "/projects/hello");
        let result = resolve_binary_from_metadata(&metadata, None, Path::new("/projects/hello"));
        assert!(result.is_ok());
        let path = result.unwrap();
        assert_eq!(path, expected_binary("/tmp/target/debug/hello"));
    }

    #[test]
    fn resolve_binary_package_not_found() {
        let metadata = sample_metadata("/tmp/target", "hello", "/projects/hello");
        let result = resolve_binary_from_metadata(
            &metadata,
            Some("nonexistent"),
            Path::new("/projects/hello"),
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("no binary target found in package 'nonexistent'")
        );
    }

    #[test]
    fn resolve_binary_no_match_by_cwd() {
        let metadata = sample_metadata("/tmp/target", "hello", "/projects/hello");
        let result = resolve_binary_from_metadata(&metadata, None, Path::new("/other/directory"));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("no binary target found in current package")
        );
    }

    #[test]
    fn resolve_binary_missing_target_directory() {
        let metadata = serde_json::json!({"packages": []});
        let result = resolve_binary_from_metadata(&metadata, None, Path::new("/tmp"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("target_directory"));
    }

    #[test]
    fn resolve_binary_missing_packages() {
        let metadata = serde_json::json!({"target_directory": "/tmp/target"});
        let result = resolve_binary_from_metadata(&metadata, None, Path::new("/tmp"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("packages"));
    }

    #[test]
    fn resolve_binary_skips_lib_targets() {
        let metadata = serde_json::json!({
            "target_directory": "/tmp/target",
            "packages": [{
                "name": "mylib",
                "manifest_path": "/projects/mylib/Cargo.toml",
                "targets": [{
                    "name": "mylib",
                    "kind": ["lib"],
                    "src_path": "/projects/mylib/src/lib.rs"
                }]
            }]
        });
        let result =
            resolve_binary_from_metadata(&metadata, Some("mylib"), Path::new("/projects/mylib"));
        assert!(result.is_err());
    }

    #[test]
    fn resolve_binary_picks_first_bin_in_multi_target() {
        let metadata = serde_json::json!({
            "target_directory": "/tmp/target",
            "packages": [{
                "name": "multi",
                "manifest_path": "/projects/multi/Cargo.toml",
                "targets": [
                    {"name": "multi", "kind": ["lib"]},
                    {"name": "server", "kind": ["bin"]},
                    {"name": "cli", "kind": ["bin"]}
                ]
            }]
        });
        let result =
            resolve_binary_from_metadata(&metadata, Some("multi"), Path::new("/projects/multi"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_binary("/tmp/target/debug/server"));
    }

    #[test]
    fn resolve_binary_with_multiple_packages() {
        let metadata = serde_json::json!({
            "target_directory": "/tmp/target",
            "packages": [
                {
                    "name": "app-a",
                    "manifest_path": "/projects/a/Cargo.toml",
                    "targets": [{"name": "app-a", "kind": ["bin"]}]
                },
                {
                    "name": "app-b",
                    "manifest_path": "/projects/b/Cargo.toml",
                    "targets": [{"name": "app-b", "kind": ["bin"]}]
                }
            ]
        });
        let result = resolve_binary_from_metadata(&metadata, Some("app-b"), Path::new("/projects"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_binary("/tmp/target/debug/app-b"));
    }

    // ── stop_server tests ──────────────────────────────────────────

    #[test]
    fn stop_server_with_none_is_noop() {
        let mut child: Option<Child> = None;
        stop_server(&mut child);
        assert!(child.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn stop_server_terminates_child() {
        // Spawn a long-running process, then stop it
        let proc = Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let mut child = Some(proc);
        stop_server(&mut child);
        assert!(child.is_none());
    }

    // ── wait_with_timeout tests ────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_succeeds_for_fast_process() {
        let mut child = Command::new("true").spawn().expect("spawn true");
        // Give it a moment to exit
        std::thread::sleep(Duration::from_millis(50));
        let result = wait_with_timeout(&mut child, Duration::from_secs(2));
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_times_out_for_long_process() {
        let mut child = Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let result = wait_with_timeout(&mut child, Duration::from_millis(100));
        assert!(result.is_err());
        // Clean up
        let _ = child.kill();
        let _ = child.wait();
    }

    // ── find_binary tests ──────────────────────────────────────────

    #[test]
    fn find_binary_resolves_workspace_package() {
        // We're running inside the autumn workspace, so this should find
        // the hello example's binary.
        let path = find_binary(Some("hello"));
        assert!(path.ends_with("debug/hello") || path.ends_with("debug/hello.exe"));
    }

    // ── constants tests ────────────────────────────────────────────

    #[test]
    fn debounce_interval_is_reasonable() {
        const { assert!(DEBOUNCE_MS >= 100, "debounce too short, would thrash") };
        const { assert!(DEBOUNCE_MS <= 5000, "debounce too long, sluggish UX") };
    }

    #[test]
    fn watch_extensions_are_non_empty() {
        assert!(!WATCH_EXTENSIONS.is_empty());
        for ext in WATCH_EXTENSIONS {
            assert!(!ext.is_empty());
            assert!(
                !ext.starts_with('.'),
                "extensions should not have leading dot"
            );
        }
    }

    #[test]
    fn watch_dirs_are_non_empty() {
        assert!(!WATCH_DIRS.is_empty());
        for dir in WATCH_DIRS {
            assert!(!dir.is_empty());
        }
    }

    #[test]
    fn watch_files_are_non_empty() {
        assert!(!WATCH_FILES.is_empty());
        for f in WATCH_FILES {
            assert!(!f.is_empty());
        }
    }
}
