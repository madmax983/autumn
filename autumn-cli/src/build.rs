//! `autumn build` -- compile the app and pre-render static routes.
//!
//! Orchestrates two steps:
//! 1. `cargo build [--release] [-p <package>]` to compile the user's binary.
//! 2. Run the binary with `AUTUMN_BUILD_STATIC=1` so the runtime renders
//!    static routes to `dist/` instead of starting the HTTP server.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Run the static build pipeline.
///
/// Note: The `run` function executes shell commands and writes to the filesystem,
/// so it is tested via integration tests in `tests/e2e.rs` rather than unit tests.
pub fn run(debug: bool, package: Option<&str>) {
    eprintln!("\u{1F342} autumn build\n");

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

    let binary = find_binary(debug, package);
    eprintln!("\nRunning static renderer...\n");

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

/// Locate the compiled binary using `cargo metadata`.
///
/// When `package` is `Some`, matches by package name directly.
/// Otherwise falls back to matching the package whose manifest is in
/// the current directory.
fn find_binary(debug: bool, package: Option<&str>) -> PathBuf {
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

    resolve_binary_from_metadata(&metadata, debug, package, &cwd).unwrap_or_else(|error| {
        eprintln!("\u{2717} {error}");
        std::process::exit(1);
    })
}

fn resolve_binary_from_metadata(
    metadata: &serde_json::Value,
    debug: bool,
    package: Option<&str>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let target_dir = metadata["target_directory"]
        .as_str()
        .ok_or("target_directory missing from cargo metadata")?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("packages missing from cargo metadata")?;

    let matching_packages: Vec<_> = package.map_or_else(
        || {
            packages
                .iter()
                .filter(|pkg| {
                    pkg["manifest_path"]
                        .as_str()
                        .and_then(|manifest| Path::new(manifest).parent())
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
        .find_map(|pkg| {
            pkg["targets"].as_array()?.iter().find_map(|t| {
                let is_bin = t["kind"].as_array()?.iter().any(|k| k == "bin");
                if is_bin {
                    t["name"].as_str().map(String::from)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            package.map_or_else(
                || "no binary target found in current package".to_owned(),
                |pkg_name| format!("no binary target found in package '{pkg_name}'"),
            )
        })?;

    let profile_dir = if debug { "debug" } else { "release" };
    let mut path = PathBuf::from(target_dir);
    path.push(profile_dir);
    path.push(bin_name);

    if cfg!(windows) {
        path.set_extension("exe");
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_binary(path: &str) -> PathBuf {
        let mut p = PathBuf::from(path);
        if cfg!(windows) {
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
            resolve_binary_from_metadata(&metadata, true, Some("hello"), Path::new("/projects"));
        assert_eq!(result.unwrap(), expected_binary("/tmp/target/debug/hello"));
    }

    #[test]
    fn resolve_binary_by_cwd() {
        let metadata = sample_metadata("/tmp/target", "hello", "/projects/hello");
        let result =
            resolve_binary_from_metadata(&metadata, true, None, Path::new("/projects/hello"));
        assert_eq!(result.unwrap(), expected_binary("/tmp/target/debug/hello"));
    }

    #[test]
    fn resolve_binary_uses_release_profile() {
        let metadata = sample_metadata("/tmp/target", "hello", "/projects/hello");
        let result =
            resolve_binary_from_metadata(&metadata, false, Some("hello"), Path::new("/projects"));
        assert_eq!(
            result.unwrap(),
            expected_binary("/tmp/target/release/hello")
        );
    }

    #[test]
    fn resolve_binary_reports_missing_package() {
        let metadata = sample_metadata("/tmp/target", "hello", "/projects/hello");
        let result =
            resolve_binary_from_metadata(&metadata, true, Some("missing"), Path::new("/projects"));
        assert!(result.unwrap_err().contains("package 'missing'"));
    }

    #[test]
    fn resolve_binary_reports_missing_targets() {
        let metadata = serde_json::json!({
            "target_directory": "/tmp/target",
            "packages": [{
                "name": "hello",
                "manifest_path": "/projects/hello/Cargo.toml",
                "targets": []
            }]
        });

        let result =
            resolve_binary_from_metadata(&metadata, true, Some("hello"), Path::new("/projects"));
        assert!(result.unwrap_err().contains("package 'hello'"));
    }
}
