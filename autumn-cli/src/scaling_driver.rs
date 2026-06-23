//! Live warm-incremental scaling measurement driver for
//! `autumn dev-loop-bench --scaling` (issue #983).
//!
//! This module scaffolds a synthetic Autumn app at each requested size N,
//! performs a warm build (untimed), then runs `--runs` timed incremental
//! rebuilds after a single-file edit (`src/handlers.rs` `BENCH_EDIT_SENTINEL`
//! bump), and feeds the timing samples to the pure report logic in
//! [`crate::dev_loop_scaling`].
//!
//! All subprocess / filesystem / timing I/O is here; the pure budget and
//! report logic is unit-tested in `dev_loop_scaling.rs`. This file is
//! excluded from coverage like `cold_start_driver.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Instant;

use crate::dev_loop_scaling::{
    GeneratedApp, apply_handler_edit, build_scaling_report, emit_scaling_report,
    generate_synthetic_app, load_baseline, parse_sizes, run_scaling_dry_run,
};

// ── Workspace location ───────────────────────────────────────────────────────

/// Locate the workspace `autumn-web` crate — reuses the same ancestor-walk
/// logic as `cold_start_driver`, made `pub(crate)` there for sharing.
fn locate_autumn_web() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AUTUMN_BENCH_AUTUMN_WEB_PATH") {
        let pb = PathBuf::from(p);
        if pb.join("Cargo.toml").is_file() {
            return Some(pb);
        }
    }
    let cwd = std::env::current_dir().ok()?;
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join("autumn");
        let manifest = candidate.join("Cargo.toml");
        if manifest.is_file()
            && let Ok(s) = std::fs::read_to_string(&manifest)
            && s.contains("name = \"autumn-web\"")
        {
            return Some(candidate);
        }
    }
    None
}

fn cached_autumn_web() -> Option<PathBuf> {
    static AUTUMN_WEB: OnceLock<Option<PathBuf>> = OnceLock::new();
    AUTUMN_WEB
        .get_or_init(|| locate_autumn_web().and_then(|p| std::fs::canonicalize(p).ok()))
        .clone()
}

// ── App scaffold ─────────────────────────────────────────────────────────────

/// Write a `GeneratedApp` to `project_dir` and append a
/// `[patch.crates-io] autumn-web` section pointing at local source.
fn scaffold_app(app: &GeneratedApp, project_dir: &Path, autumn_web: &Path) -> Result<(), String> {
    // Write Cargo.toml + [patch]
    let patch = format!(
        "\n[patch.crates-io]\nautumn-web = {{ path = {:?} }}\n",
        autumn_web.display().to_string()
    );
    std::fs::write(project_dir.join("Cargo.toml"), format!("{}{patch}", app.cargo_toml))
        .map_err(|e| format!("write Cargo.toml: {e}"))?;

    // Write source files
    for (relpath, content) in &app.files {
        let dest = project_dir.join(relpath);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        std::fs::write(&dest, content).map_err(|e| format!("write {}: {e}", dest.display()))?;
    }

    Ok(())
}

// ── Build helpers ────────────────────────────────────────────────────────────

/// Run `cargo build` in `project_dir`, capturing stderr for diagnostics.
///
/// Pass `timed = true` to wrap the call in an `Instant` and return elapsed ms;
/// `timed = false` returns `0` (warm-up build that is not measured).
fn run_cargo_build(project_dir: &Path, timed: bool) -> Result<u64, String> {
    let target_dir = project_dir.join("target");
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--target-dir"])
        .arg(&target_dir)
        .current_dir(project_dir)
        // Remove inherited target-dir overrides so the warm cache for THIS
        // project accumulates in its own dedicated directory.
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        // Suppress output so bench stderr is clean.
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let start = Instant::now();
    let status = cmd
        .status()
        .map_err(|e| format!("cargo build spawn failed: {e}"))?;
    let elapsed = if timed {
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
    } else {
        0
    };

    if !status.success() {
        return Err(format!("cargo build failed (exit: {status})"));
    }
    Ok(elapsed)
}

// ── Per-size measurement ─────────────────────────────────────────────────────

/// Measure warm incremental rebuild latency for a synthetic app of size `n`.
///
/// 1. Scaffolds the app in a fresh tempdir.
/// 2. Runs one **warm build** (untimed) to populate the incremental cache.
/// 3. Runs `runs` **timed builds**, each preceded by bumping the
///    `BENCH_EDIT_SENTINEL` in `src/handlers.rs` to force recompilation of
///    exactly that one file.
///
/// Returns a `Vec<u64>` of `runs` timing samples in milliseconds.
fn measure_size(n: usize, runs: u32, autumn_web: &Path) -> Result<Vec<u64>, String> {
    let tmp = tempfile::tempdir().map_err(|e| format!("create temp dir: {e}"))?;
    let project_dir = tmp.path().to_path_buf();

    let app = generate_synthetic_app(n);
    scaffold_app(&app, &project_dir, autumn_web)?;

    eprintln!("  N={n}: warm build (untimed)…");
    run_cargo_build(&project_dir, false)?;

    let handlers_path = project_dir.join("src/handlers.rs");
    let mut samples = Vec::with_capacity(runs as usize);

    for run in 1..=runs {
        // Bump the sentinel to trigger a one-file incremental recompile.
        let handlers_src = std::fs::read_to_string(&handlers_path)
            .map_err(|e| format!("read handlers.rs: {e}"))?;
        let edited = apply_handler_edit(&handlers_src, run);
        std::fs::write(&handlers_path, &edited)
            .map_err(|e| format!("write handlers.rs: {e}"))?;

        eprintln!("  N={n} run {run}/{runs}: timed incremental build…");
        let ms = run_cargo_build(&project_dir, true)?;
        eprintln!("    → {ms} ms");
        samples.push(ms);
    }

    Ok(samples)
}

// ── Sweep ────────────────────────────────────────────────────────────────────

/// Run the full scaling sweep: for each size in `sizes`, measure `runs`
/// incremental rebuild samples and return `(n, samples)` pairs.
fn measure_scaling(
    sizes: &[usize],
    runs: u32,
    autumn_web: &Path,
) -> Result<Vec<(usize, Vec<u64>)>, String> {
    let mut results = Vec::with_capacity(sizes.len());
    for &n in sizes {
        eprintln!("Measuring N={n}…");
        let samples = measure_size(n, runs, autumn_web)?;
        results.push((n, samples));
    }
    Ok(results)
}

// ── CLI entry point ──────────────────────────────────────────────────────────

/// Run the `autumn dev-loop-bench --scaling` command.
///
/// In `--dry-run` mode prints the budget table without building anything.
/// Otherwise locates the workspace `autumn-web`, runs the sizing sweep, and
/// emits a scaling report.
#[allow(clippy::fn_params_excessive_bools)]
pub fn run_scaling(
    sizes_csv: &str,
    runs: u32,
    output: Option<&str>,
    json: bool,
    fail_on_regression: bool,
    dry_run: bool,
    baseline_path: Option<&str>,
) -> i32 {
    if dry_run {
        return run_scaling_dry_run();
    }

    if runs == 0 {
        eprintln!("Error: --runs must be at least 1 for a scaling measurement.");
        return 1;
    }

    let sizes = match parse_sizes(sizes_csv) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: invalid --sizes: {e}");
            return 1;
        }
    };

    let Some(autumn_web) = cached_autumn_web() else {
        eprintln!(
            "Error: could not locate the workspace autumn-web crate; \
             set AUTUMN_BENCH_AUTUMN_WEB_PATH to its directory."
        );
        return 1;
    };

    eprintln!(
        "autumn dev-loop-bench --scaling: warming and measuring {} size(s), {} run(s) each",
        sizes.len(),
        runs
    );
    eprintln!(
        "This scaffolds a synthetic Autumn app for each N and runs timed incremental builds — \
         expect several minutes total.\n"
    );

    let measurements = match measure_scaling(&sizes, runs, &autumn_web) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error: scaling measurement failed: {e}");
            return 1;
        }
    };

    let baseline = load_baseline(baseline_path);
    let report = build_scaling_report(&measurements, baseline.as_ref());
    emit_scaling_report(&report, json, output, fail_on_regression)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_autumn_web_honours_env_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("Cargo.toml"), "name = \"autumn-web\"\n")
            .expect("write manifest");
        let found = temp_env::with_var(
            "AUTUMN_BENCH_AUTUMN_WEB_PATH",
            Some(tmp.path().as_os_str()),
            locate_autumn_web,
        );
        assert_eq!(found.as_deref(), Some(tmp.path()));
    }

    #[test]
    fn scaffold_app_writes_cargo_toml_and_sources() {
        use crate::dev_loop_scaling::generate_synthetic_app;
        let tmp = tempfile::tempdir().expect("tempdir");
        let autumn_web = tmp.path().join("autumn-web");
        std::fs::create_dir_all(&autumn_web).expect("mkdir");

        let app = generate_synthetic_app(2);
        scaffold_app(&app, tmp.path(), &autumn_web).expect("scaffold must succeed");

        assert!(tmp.path().join("Cargo.toml").is_file(), "Cargo.toml must exist");
        let manifest =
            std::fs::read_to_string(tmp.path().join("Cargo.toml")).expect("read manifest");
        assert!(manifest.contains("[patch.crates-io]"), "patch section must be appended");

        for (relpath, _) in &app.files {
            assert!(
                tmp.path().join(relpath).is_file(),
                "{relpath} must be written"
            );
        }
    }

    #[test]
    fn run_scaling_dry_run_returns_zero() {
        assert_eq!(run_scaling("1,25,50,100", 3, None, false, false, true, None), 0);
    }

    #[test]
    fn run_scaling_rejects_zero_runs() {
        let exit = run_scaling("1,100", 0, None, false, false, false, None);
        assert_eq!(exit, 1, "zero runs must be rejected");
    }

    #[test]
    fn run_scaling_rejects_invalid_sizes() {
        let exit = run_scaling("abc", 1, None, false, false, false, None);
        assert_eq!(exit, 1, "invalid sizes must be rejected");
    }

    // Live test — compiles a real n=1 synthetic app; gated behind --ignored.
    #[test]
    #[ignore = "compiles a synthetic app from scratch; run with --ignored"]
    fn scaling_live_n1_measures_positive_duration() {
        let autumn_web = cached_autumn_web().expect("must locate autumn-web");
        let samples = measure_size(1, 1, &autumn_web).expect("must succeed");
        assert_eq!(samples.len(), 1);
        assert!(samples[0] > 0, "measured duration must be positive");
    }
}
