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
pub fn run(package: Option<&str>) {
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
                let dominated_by_relevant =
                    events.iter().any(|e| is_relevant_change(&e.path, e.kind));

                if !dominated_by_relevant {
                    continue;
                }

                let changed: Vec<_> = events
                    .iter()
                    .filter(|e| is_relevant_change(&e.path, e.kind))
                    .map(|e| e.path.display().to_string())
                    .collect();

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

/// Run `cargo build` for the given package. Returns true on success.
fn cargo_build(package: Option<&str>) -> bool {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if let Some(pkg) = package {
        cmd.args(["-p", pkg]);
    }

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

/// Locate the compiled binary using `cargo metadata`.
///
/// Reuses the same approach as `build.rs` but always targets the debug
/// profile since `autumn dev` is for development.
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

    let target_dir = metadata["target_directory"]
        .as_str()
        .expect("target_directory in metadata");

    let packages = metadata["packages"].as_array().expect("packages array");

    let matching_packages: Vec<_> = package.map_or_else(
        || {
            let cwd = std::env::current_dir().expect("current dir");
            packages
                .iter()
                .filter(|pkg| {
                    let manifest = pkg["manifest_path"].as_str().unwrap_or("");
                    std::path::Path::new(manifest)
                        .parent()
                        .is_some_and(|dir| dir.starts_with(&cwd))
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
        .unwrap_or_else(|| {
            if let Some(pkg_name) = package {
                eprintln!("\u{2717} No binary target found in package '{pkg_name}'");
            } else {
                eprintln!("\u{2717} No binary target found in current package");
                eprintln!("  Hint: use -p <package> to specify the target package");
            }
            std::process::exit(1);
        });

    let mut path = PathBuf::from(target_dir);
    path.push("debug");
    path.push(&bin_name);

    if cfg!(target_os = "windows") {
        path.set_extension("exe");
    }

    path
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
