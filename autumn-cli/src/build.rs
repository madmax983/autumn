//! `autumn build` -- compile the app and pre-render static routes.
//!
//! Orchestrates two steps:
//! 1. `cargo build [--release] [-p <package>]` to compile the user's binary.
//! 2. Run the binary with `AUTUMN_BUILD_STATIC=1` -- the runtime
//!    detects this and renders static routes to `dist/` instead of
//!    starting the HTTP server.

use std::process::Command;

/// Run the static build pipeline.
pub fn run(debug: bool, package: Option<&str>) {
    eprintln!("\u{1F342} autumn build\n");

    // Step 1: Compile
    let profile = if debug { "dev" } else { "release" };
    let mut cargo = Command::new("cargo");
    cargo.arg("build");
    if !debug {
        cargo.arg("--release");
    }
    if let Some(pkg) = package {
        cargo.args(["-p", pkg]);
    }

    eprintln!("Compiling ({profile} profile)...");
    let status = cargo.status().expect("failed to run cargo build");
    if !status.success() {
        eprintln!("\u{2717} Compilation failed");
        std::process::exit(1);
    }

    if has_wasm_client(package) {
        build_wasm_bundle(debug, package);
    }

    // Step 2: Find the binary
    let binary = find_binary(debug, package);
    eprintln!("\nRunning static renderer...\n");

    // Step 3: Run with AUTUMN_BUILD_STATIC=1
    let status = Command::new(&binary)
        .env("AUTUMN_BUILD_STATIC", "1")
        .status()
        .unwrap_or_else(|e| {
            eprintln!("\u{2717} Failed to run {}: {e}", binary.display());
            std::process::exit(1);
        });

    if !status.success() {
        eprintln!("\n\u{2717} Static build failed");
        std::process::exit(1);
    }

    eprintln!("\n\u{1F342} Build complete!");
}

fn has_wasm_client(package: Option<&str>) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return std::path::Path::new("src/client.rs").exists();
    };
    let Some(metadata) = try_cargo_metadata() else {
        return std::path::Path::new("src/client.rs").exists();
    };

    resolve_wasm_client_target_from_metadata(&metadata, package, &cwd).is_ok()
}

fn build_wasm_bundle(debug: bool, package: Option<&str>) {
    let Ok(cwd) = std::env::current_dir() else {
        eprintln!(
            "  Warning: skipping WASM client build because the current directory is unavailable"
        );
        return;
    };

    let Some(metadata) = try_cargo_metadata() else {
        eprintln!("  Warning: skipping WASM client build because cargo metadata was unavailable");
        return;
    };

    let client_target = match resolve_wasm_client_target_from_metadata(&metadata, package, &cwd) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("  Warning: skipping WASM client build: {error}");
            return;
        }
    };

    eprintln!("Compiling WASM client entry...");
    let mut cargo = Command::new("cargo");
    cargo.args(["build", "--target", "wasm32-unknown-unknown", "--bin"]);
    cargo.arg(&client_target);
    if !debug {
        cargo.arg("--release");
    }
    if let Some(pkg) = package {
        cargo.args(["-p", pkg]);
    }

    let status = cargo.status().expect("failed to run cargo wasm build");
    if !status.success() {
        eprintln!("\u{2717} WASM client compilation failed");
        std::process::exit(1);
    }
}

fn resolve_wasm_client_target_from_metadata(
    metadata: &serde_json::Value,
    package: Option<&str>,
    cwd: &std::path::Path,
) -> Result<String, String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("missing packages array in metadata")?;

    let package_entry = package.map_or_else(
        || {
            packages.iter().find(|pkg| {
                pkg["manifest_path"]
                    .as_str()
                    .and_then(|manifest| std::path::Path::new(manifest).parent())
                    .is_some_and(|dir| dir.starts_with(cwd))
            })
        },
        |pkg_name| {
            packages
                .iter()
                .find(|pkg| pkg["name"].as_str() == Some(pkg_name))
        },
    );

    let package_entry = package_entry.ok_or_else(|| {
        package.map_or_else(
            || "current package not found in cargo metadata".to_owned(),
            |pkg_name| format!("package '{pkg_name}' not found in cargo metadata"),
        )
    })?;

    package_entry["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
                && target["src_path"].as_str().is_some_and(|src| {
                    std::path::Path::new(src)
                        .file_name()
                        .is_some_and(|file| file == "client.rs")
                })
        })
        .and_then(|target| target["name"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            package.map_or_else(
                || {
                    "current package has no `src/client.rs` binary target; add [[bin]] name = \"client\" path = \"src/client.rs\""
                        .to_owned()
                },
                |pkg_name| {
                    format!(
                        "package '{pkg_name}' has no `src/client.rs` binary target; add [[bin]] name = \"client\" path = \"src/client.rs\""
                    )
                },
            )
        })
}

fn try_cargo_metadata() -> Option<serde_json::Value> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Locate the compiled binary using `cargo metadata`.
///
/// When `package` is `Some`, matches by package name directly.
/// Otherwise falls back to matching the package whose manifest is in
/// the current directory.
fn find_binary(debug: bool, package: Option<&str>) -> std::path::PathBuf {
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

    // Filter packages: by name if -p was given, otherwise by cwd
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

    let profile_dir = if debug { "debug" } else { "release" };
    let mut path = std::path::PathBuf::from(target_dir);
    path.push(profile_dir);
    path.push(&bin_name);

    if cfg!(target_os = "windows") {
        path.set_extension("exe");
    }

    path
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn wasm_build_targets_client_bin_instead_of_lib() {
        let _guard = test_lock().lock().expect("lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let script_dir = temp.path().join("bin");
        fs::create_dir_all(&script_dir).expect("create bin dir");
        let log_path = temp.path().join("cargo-args.log");
        let fake_cargo = write_fake_cargo(&script_dir, &log_path, temp.path());
        let original_path = std::env::var_os("PATH");
        let _env = EnvGuard::set_many(&[
            (
                "PATH",
                Some(prepend_path(
                    fake_cargo.parent().expect("script parent"),
                    original_path.as_deref(),
                )),
            ),
            (
                "AUTUMN_FAKE_CARGO_LOG",
                Some(log_path.as_os_str().to_os_string()),
            ),
        ]);
        let _cwd = CwdGuard::change(temp.path());

        build_wasm_bundle(true, Some("demo-app"));

        let log = fs::read_to_string(&log_path).expect("read cargo log");
        let wasm_invocation = log
            .lines()
            .find(|line| line.contains("--target wasm32-unknown-unknown"))
            .expect("wasm cargo invocation");

        assert!(
            wasm_invocation.contains("--bin client"),
            "WASM bundle build should compile the client entry binary: {wasm_invocation}",
        );
        assert!(
            !wasm_invocation.contains("--lib"),
            "WASM bundle build should not use the library target: {wasm_invocation}",
        );
    }

    fn write_fake_cargo(dir: &Path, log_path: &Path, project_dir: &Path) -> PathBuf {
        #[cfg(windows)]
        let script_path = dir.join("cargo.cmd");
        #[cfg(not(windows))]
        let script_path = dir.join("cargo");

        let target_dir = escape_json_path(&dir.join("..").join("target"));
        let project_dir = escape_json_path(project_dir);

        #[cfg(windows)]
        let script = format!(
            "@echo off\r\nsetlocal\r\n>>\"{}\" echo %*\r\nif \"%1\"==\"metadata\" (\r\n  echo {{\"target_directory\":\"{}\",\"packages\":[{{\"name\":\"demo-app\",\"manifest_path\":\"{}\\\\Cargo.toml\",\"targets\":[{{\"name\":\"demo-app\",\"kind\":[\"bin\"],\"src_path\":\"{}\\\\src\\\\main.rs\"}},{{\"name\":\"client\",\"kind\":[\"bin\"],\"src_path\":\"{}\\\\src\\\\client.rs\"}}]}}]}}\r\n)\r\nexit /b 0\r\n",
            log_path.display(),
            target_dir,
            project_dir,
            project_dir,
            project_dir,
        );

        #[cfg(not(windows))]
        let script = format!(
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"${{1:-}}\" = \"metadata\" ]; then\n  printf '%s\\n' '{{\"target_directory\":\"{}\",\"packages\":[{{\"name\":\"demo-app\",\"manifest_path\":\"{}/Cargo.toml\",\"targets\":[{{\"name\":\"demo-app\",\"kind\":[\"bin\"],\"src_path\":\"{}/src/main.rs\"}},{{\"name\":\"client\",\"kind\":[\"bin\"],\"src_path\":\"{}/src/client.rs\"}}]}}]}}'\nfi\nexit 0\n",
            log_path.display(),
            target_dir,
            project_dir,
            project_dir,
            project_dir,
        );

        fs::write(&script_path, script).expect("write fake cargo");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script_path, permissions).expect("chmod");
        }

        script_path
    }

    fn prepend_path(dir: &Path, existing: Option<&OsStr>) -> OsString {
        let mut paths = vec![dir.to_path_buf()];
        if let Some(existing) = existing {
            paths.extend(std::env::split_paths(existing));
        }
        std::env::join_paths(paths).expect("join PATH")
    }

    fn escape_json_path(path: &Path) -> String {
        path.display().to_string().replace('\\', "\\\\")
    }

    struct EnvGuard {
        saved: Vec<(String, Option<OsString>)>,
    }

    impl EnvGuard {
        fn set_many(pairs: &[(&str, Option<OsString>)]) -> Self {
            let mut saved = Vec::with_capacity(pairs.len());
            for (key, value) in pairs {
                saved.push(((*key).to_owned(), std::env::var_os(key)));
                match value {
                    Some(value) => {
                        // SAFETY: test-only environment changes are serialized with a process-wide mutex.
                        unsafe { std::env::set_var(key, value) };
                    }
                    None => {
                        // SAFETY: test-only environment changes are serialized with a process-wide mutex.
                        unsafe { std::env::remove_var(key) };
                    }
                }
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..).rev() {
                match value {
                    Some(value) => {
                        // SAFETY: test-only environment changes are serialized with a process-wide mutex.
                        unsafe { std::env::set_var(&key, value) };
                    }
                    None => {
                        // SAFETY: test-only environment changes are serialized with a process-wide mutex.
                        unsafe { std::env::remove_var(&key) };
                    }
                }
            }
        }
    }

    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn change(path: &Path) -> Self {
            let original = std::env::current_dir().expect("cwd");
            std::env::set_current_dir(path).expect("change cwd");
            Self { original }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.original).expect("restore cwd");
        }
    }
}
