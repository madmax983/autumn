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
    let mut cargo = build_wasm_command(debug, package, &client_target);

    let status = cargo.status().expect("failed to run cargo wasm build");
    if !status.success() {
        eprintln!("\u{2717} WASM client compilation failed");
        std::process::exit(1);
    }
}

fn build_wasm_command(debug: bool, package: Option<&str>, client_target: &str) -> Command {
    let mut cargo = Command::new("cargo");
    cargo.args(["build", "--target", "wasm32-unknown-unknown", "--bin"]);
    cargo.arg(client_target);
    if !debug {
        cargo.arg("--release");
    }
    if let Some(pkg) = package {
        cargo.args(["-p", pkg]);
    }
    cargo
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn wasm_build_command_targets_client_bin_instead_of_lib() {
        let command = build_wasm_command(true, Some("demo-app"), "client");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(command.get_program().to_string_lossy(), "cargo");
        assert_eq!(
            args,
            vec![
                "build",
                "--target",
                "wasm32-unknown-unknown",
                "--bin",
                "client",
                "-p",
                "demo-app",
            ],
        );
        assert!(
            !args.iter().any(|arg| arg == "--lib"),
            "WASM bundle build should not use the library target: {args:?}",
        );
    }

    #[test]
    fn wasm_build_command_adds_release_flag_for_release_builds() {
        let command = build_wasm_command(false, None, "client");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(
            args.iter().any(|arg| arg == "--release"),
            "release WASM builds should pass --release: {args:?}",
        );
    }

    #[test]
    fn resolve_wasm_client_target_by_package_name() {
        let metadata = serde_json::json!({
            "packages": [
                {
                    "name": "alpha",
                    "manifest_path": "/repo/alpha/Cargo.toml",
                    "targets": [
                        {
                            "name": "client",
                            "kind": ["bin"],
                            "src_path": "/repo/alpha/src/client.rs"
                        }
                    ]
                },
                {
                    "name": "beta",
                    "manifest_path": "/repo/beta/Cargo.toml",
                    "targets": [
                        {
                            "name": "server",
                            "kind": ["bin"],
                            "src_path": "/repo/beta/src/main.rs"
                        }
                    ]
                }
            ]
        });

        let target =
            resolve_wasm_client_target_from_metadata(&metadata, Some("alpha"), Path::new("/repo"))
                .expect("resolve wasm client target");

        assert_eq!(target, "client");
    }

    #[test]
    fn resolve_wasm_client_target_by_current_directory() {
        let metadata = serde_json::json!({
            "packages": [
                {
                    "name": "outside",
                    "manifest_path": "/elsewhere/outside/Cargo.toml",
                    "targets": [
                        {
                            "name": "client",
                            "kind": ["bin"],
                            "src_path": "/elsewhere/outside/src/client.rs"
                        }
                    ]
                },
                {
                    "name": "inside",
                    "manifest_path": "/repo/app/Cargo.toml",
                    "targets": [
                        {
                            "name": "client-app",
                            "kind": ["bin"],
                            "src_path": "/repo/app/src/client.rs"
                        }
                    ]
                }
            ]
        });

        let target =
            resolve_wasm_client_target_from_metadata(&metadata, None, Path::new("/repo/app"))
                .expect("resolve wasm client target");

        assert_eq!(target, "client-app");
    }

    #[test]
    fn resolve_wasm_client_target_reports_missing_client_bin() {
        let metadata = serde_json::json!({
            "packages": [
                {
                    "name": "demo",
                    "manifest_path": "/repo/demo/Cargo.toml",
                    "targets": [
                        {
                            "name": "server",
                            "kind": ["bin"],
                            "src_path": "/repo/demo/src/main.rs"
                        }
                    ]
                }
            ]
        });

        let error = resolve_wasm_client_target_from_metadata(
            &metadata,
            Some("demo"),
            Path::new("/repo/demo"),
        )
        .expect_err("missing client target should error");

        assert!(error.contains("package 'demo'"));
        assert!(error.contains("src/client.rs"));
    }
}
