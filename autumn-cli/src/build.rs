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
