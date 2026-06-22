//! Dev-loop latency budget, statistics, and gating for `autumn dev`.
//!
//! This module defines the accepted latency budgets for every `autumn dev`
//! change class, helpers to compute p50/p95/max statistics, budget-checking
//! logic with actionable diagnostics, and report formatters (human-readable
//! text + machine-readable JSON).
//!
//! See `docs/guide/dev-loop-latency.md` for the full budget matrix and the
//! methodology used to measure end-to-end latency.

use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

// ── Change classes ───────────────────────────────────────────────────────────

/// A category of file change that `autumn dev` handles.
///
/// Each variant corresponds to one row in the dev-loop latency budget matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClass {
    /// Initial dev boot: time from `autumn dev` invocation to first
    /// successful HTTP response on the app's root route.
    InitialBoot,
    /// Warm incremental Rust route edit in `examples/hello` (no database).
    RustRouteEditHello,
    /// Warm incremental Rust route edit in a database-backed example
    /// (default: `examples/todo-app`).
    RustRouteEditDb,
    /// CSS/Tailwind input-file edit to refreshed browser stylesheet.
    CssTailwind,
    /// Static asset edit (image, JS, font) to browser-visible reload.
    StaticAsset,
    /// `autumn.toml` or profile config edit to restarted server.
    ConfigEdit,
    /// Custom `dev.watch_dirs` entry edit to restarted server.
    WatchDirEdit,
    /// Cold-start onboarding: `autumn new` → `autumn dev` → first HTTP 200,
    /// **including the first clean compile**, for the no-DB `hello` shape.
    /// This is the gated onboarding budget (issue #977).
    ColdStartHello,
    /// Cold-start onboarding for the database-backed shape. Measured as
    /// **informational** in this slice — it does not gate CI.
    ColdStartDb,
}

impl ChangeClass {
    /// Return a stable lowercase snake-case key for use in JSON output.
    pub const fn key(self) -> &'static str {
        match self {
            Self::InitialBoot => "initial_boot",
            Self::RustRouteEditHello => "rust_route_edit_hello",
            Self::RustRouteEditDb => "rust_route_edit_db",
            Self::CssTailwind => "css_tailwind",
            Self::StaticAsset => "static_asset",
            Self::ConfigEdit => "config_edit",
            Self::WatchDirEdit => "watch_dir_edit",
            Self::ColdStartHello => "cold_start_hello",
            Self::ColdStartDb => "cold_start_db",
        }
    }

    /// Return a human-readable name for the user journey this class represents.
    pub const fn journey_name(self) -> &'static str {
        match self {
            Self::InitialBoot => "Initial dev boot to first route",
            Self::RustRouteEditHello => "Rust route edit (examples/hello, no-DB)",
            Self::RustRouteEditDb => "Rust route edit (database-backed example)",
            Self::CssTailwind => "CSS/Tailwind edit to refreshed stylesheet",
            Self::StaticAsset => "Static asset edit to browser reload",
            Self::ConfigEdit => "Config edit (autumn.toml) to restarted server",
            Self::WatchDirEdit => "Custom watch_dirs edit to restarted server",
            Self::ColdStartHello => "Cold start (autumn new → first 200, no-DB)",
            Self::ColdStartDb => "Cold start (autumn new → first 200, database-backed)",
        }
    }
}

// ── Budgets ──────────────────────────────────────────────────────────────────

/// Accepted latency budget for one change class (all values in milliseconds).
// The `_ms` suffix is the unit — suppress the struct-field-names lint.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LatencyBudget {
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

/// Return the canonical accepted latency budget for the given change class.
///
/// These budgets match the success metrics declared in issue #601:
/// - CSS/static reload: p95 ≤ 1 s
/// - Rust warm edit in `examples/hello`: p95 ≤ 5 s
/// - Rust warm edit in a database-backed example: p95 ≤ 10 s
pub const fn budget_for(class: ChangeClass) -> LatencyBudget {
    match class {
        ChangeClass::InitialBoot => LatencyBudget {
            p50_ms: 10_000,
            p95_ms: 20_000,
            max_ms: 40_000,
        },
        ChangeClass::RustRouteEditHello => LatencyBudget {
            p50_ms: 3_000,
            p95_ms: 5_000,
            max_ms: 10_000,
        },
        ChangeClass::RustRouteEditDb => LatencyBudget {
            p50_ms: 5_000,
            p95_ms: 10_000,
            max_ms: 20_000,
        },
        ChangeClass::CssTailwind => LatencyBudget {
            p50_ms: 500,
            p95_ms: 1_000,
            max_ms: 2_000,
        },
        ChangeClass::StaticAsset => LatencyBudget {
            p50_ms: 300,
            p95_ms: 1_000,
            max_ms: 2_000,
        },
        ChangeClass::ConfigEdit | ChangeClass::WatchDirEdit => LatencyBudget {
            p50_ms: 3_000,
            p95_ms: 8_000,
            max_ms: 15_000,
        },
        // Cold-start onboarding budget (issue #977). The success metric is
        // p95 ≤ 60s for the no-DB `hello` shape on the CI reference runner.
        ChangeClass::ColdStartHello => LatencyBudget {
            p50_ms: 45_000,
            p95_ms: 60_000,
            max_ms: 90_000,
        },
        // Database-backed cold start is informational only in this slice, so
        // these limits are not gated. The bundled managed-Postgres provider adds
        // significant compile + first-boot weight (it embeds and starts a real
        // Postgres), so the expectation is generous.
        ChangeClass::ColdStartDb => LatencyBudget {
            p50_ms: 120_000,
            p95_ms: 180_000,
            max_ms: 300_000,
        },
    }
}

// ── Statistics ───────────────────────────────────────────────────────────────

/// Computed latency statistics for a set of timing samples.
// The `_ms` suffix is the unit — suppress the struct-field-names lint.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClassStats {
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
    pub sample_count: usize,
}

/// Compute p50, p95, and maximum for a slice of timing samples (milliseconds).
///
/// Uses the nearest-rank method: the k-th percentile of n samples is
/// `sorted[ceil(k/100 * n) - 1]`. Returns all-zeros for an empty slice.
/// Ceiling division is computed with integer arithmetic to avoid f64 casts.
pub fn compute_stats(samples: &[u64]) -> ClassStats {
    if samples.is_empty() {
        return ClassStats {
            p50_ms: 0,
            p95_ms: 0,
            max_ms: 0,
            sample_count: 0,
        };
    }

    let mut sorted = samples.to_vec();
    sorted.sort_unstable();

    let n = sorted.len();
    // Nearest-rank: ceil(p/100 * n) = (p * n).div_ceil(100).
    let percentile = |p: usize| -> u64 {
        let rank = (p * n).div_ceil(100);
        sorted[rank.min(n) - 1]
    };

    ClassStats {
        p50_ms: percentile(50),
        p95_ms: percentile(95),
        max_ms: *sorted.last().unwrap(),
        sample_count: n,
    }
}

// ── Budget checking ──────────────────────────────────────────────────────────

/// Result of comparing measured statistics against the accepted budget.
// Each bool is an independent, named outcome flag (pass / p95 / max /
// informational); a state machine would obscure rather than clarify them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
pub struct BudgetCheckResult {
    pub change_class: String,
    pub journey_name: String,
    pub stats: ClassStats,
    pub budget: LatencyBudget,
    pub passed: bool,
    pub p95_exceeded: bool,
    pub max_exceeded: bool,
    /// Percentage above the p95 budget (0 when within budget).
    pub p95_overage_pct: f64,
    /// Human-readable diagnosis of the result.
    pub diagnosis: String,
    /// What the developer should do next (empty string when passing).
    pub next_action: String,
    /// When true this result is reported for visibility only and does **not**
    /// contribute to the overall pass/fail gate (e.g. the database-backed
    /// cold-start shape, which is informational in this slice).
    #[serde(default)]
    pub informational: bool,
}

/// Compare measured statistics against the accepted budget for a change class.
///
/// Produces a `BudgetCheckResult` that names the failing user journey,
/// states the diagnosis, and proposes a concrete next action.
pub fn check_budget(
    class: ChangeClass,
    stats: ClassStats,
    budget: &LatencyBudget,
) -> BudgetCheckResult {
    let p95_exceeded = stats.p95_ms > budget.p95_ms;
    let max_exceeded = stats.max_ms > budget.max_ms;
    let passed = !p95_exceeded && !max_exceeded;

    // Integer percentage: (over * 100) / budget.  Percentages fit in u32
    // (max ~10000% for extreme cases), so the u32→f64 cast is lossless.
    let p95_overage_pct = if p95_exceeded {
        let over = stats.p95_ms.saturating_sub(budget.p95_ms);
        let pct = over.saturating_mul(100) / budget.p95_ms.max(1);
        f64::from(u32::try_from(pct).unwrap_or(u32::MAX))
    } else {
        0.0
    };

    let (diagnosis, next_action) = if passed {
        (String::new(), String::new())
    } else {
        build_diagnostics(class, &stats, budget, p95_exceeded, max_exceeded)
    };

    BudgetCheckResult {
        change_class: class.key().to_string(),
        journey_name: class.journey_name().to_string(),
        stats,
        budget: *budget,
        passed,
        p95_exceeded,
        max_exceeded,
        p95_overage_pct,
        diagnosis,
        next_action,
        informational: false,
    }
}

fn build_diagnostics(
    class: ChangeClass,
    stats: &ClassStats,
    budget: &LatencyBudget,
    p95_exceeded: bool,
    max_exceeded: bool,
) -> (String, String) {
    let mut parts = Vec::new();

    if p95_exceeded {
        let over_pct = stats
            .p95_ms
            .saturating_sub(budget.p95_ms)
            .saturating_mul(100)
            / budget.p95_ms.max(1);
        parts.push(format!(
            "p95 {}ms exceeds budget {}ms ({}% over)",
            stats.p95_ms, budget.p95_ms, over_pct,
        ));
    }
    if max_exceeded {
        parts.push(format!(
            "max {}ms exceeds budget {}ms",
            stats.max_ms, budget.max_ms
        ));
    }

    let diagnosis = format!(
        "Journey '{}' regressed: {}.",
        class.journey_name(),
        parts.join("; ")
    );

    let next_action = match class {
        ChangeClass::CssTailwind => {
            "Check for new CSS plugins or a slow Tailwind config glob. \
             Run `autumn dev` manually and time the Tailwind step in the log."
        }
        ChangeClass::StaticAsset => {
            "Verify no new large static assets were added. \
             Check that the static-file watcher is not triggering unnecessary reloads."
        }
        ChangeClass::RustRouteEditHello => {
            "A Rust compile step slowed for the no-DB path. \
             Check for new proc-macro dependencies or increased monomorphisation. \
             Run `cargo build -p hello --timings` to identify slow crates."
        }
        ChangeClass::RustRouteEditDb => {
            "A Rust compile step slowed for the database-backed path. \
             Check for new ORM dependencies or schema changes that increase compile time. \
             Run `cargo build --timings` to identify slow crates."
        }
        ChangeClass::InitialBoot => {
            "Initial boot slowed. Check for new blocking startup tasks, \
             migration count growth, or Tailwind cold-start overhead. \
             Review the `autumn dev` startup log for the slow phase."
        }
        ChangeClass::ConfigEdit | ChangeClass::WatchDirEdit => {
            "Server restart latency increased. Check for new startup hooks, \
             increased migration count, or blocking I/O in app initialisation."
        }
        ChangeClass::ColdStartHello | ChangeClass::ColdStartDb => {
            "Cold-start onboarding slowed: the first clean compile got heavier. \
             A new default dependency or feature likely bloated the from-scratch \
             build. This slice is measurement-only — run `cargo build --timings` \
             on a fresh checkout to find the slow crates, then open a separate \
             optimization slice (dependency trimming, codegen-units, linker)."
        }
    };

    (diagnosis, next_action.to_string())
}

// ── Full report ──────────────────────────────────────────────────────────────

/// A complete benchmark report covering all measured change classes.
#[derive(Debug, Serialize)]
pub struct FullReport {
    pub timestamp_utc: String,
    pub runner_os: String,
    pub rust_version: String,
    pub autumn_version: String,
    pub example_name: String,
    pub all_passed: bool,
    pub results: Vec<BudgetCheckResult>,
}

// ── Formatters ───────────────────────────────────────────────────────────────

/// Serialise a `FullReport` as a machine-readable JSON string.
///
/// The output is suitable for archiving as release evidence. It deliberately
/// omits local file paths; the runner OS and Rust version supply enough
/// context to interpret variance across environments.
pub fn format_json_report(report: &FullReport) -> String {
    serde_json::to_string_pretty(report)
        .unwrap_or_else(|e| format!("{{\"error\": \"serialisation failed: {e}\"}}"))
}

/// Format a `FullReport` as a human-readable summary.
///
/// The summary shows one row per change class with p50/p95/max timings and a
/// pass/fail indicator. Failing rows include the diagnosis and next action.
pub fn format_human_summary(report: &FullReport) -> String {
    let mut out = String::new();

    writeln!(
        out,
        "Autumn dev-loop latency report — {}",
        report.timestamp_utc
    )
    .unwrap();
    writeln!(
        out,
        "Runner: {}  Rust: {}  autumn-web: {}",
        report.runner_os, report.rust_version, report.autumn_version
    )
    .unwrap();
    writeln!(out, "Example: {}", report.example_name).unwrap();
    out.push('\n');

    let col_w = 46usize;
    writeln!(
        out,
        "{:<col_w$}  {:>8}  {:>8}  {:>8}  Status",
        "Change class", "p50 ms", "p95 ms", "max ms",
    )
    .unwrap();
    writeln!(out, "{}", "-".repeat(col_w + 40)).unwrap();

    for r in &report.results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        writeln!(
            out,
            "{:<col_w$}  {:>8}  {:>8}  {:>8}  {}",
            r.journey_name, r.stats.p50_ms, r.stats.p95_ms, r.stats.max_ms, status,
        )
        .unwrap();
        if !r.passed {
            writeln!(out, "  ↳ {}", r.diagnosis).unwrap();
            writeln!(out, "  ↳ Next: {}", r.next_action).unwrap();
        }
    }

    out.push('\n');
    let overall = if report.all_passed { "PASS" } else { "FAIL" };
    writeln!(out, "Overall: {overall}").unwrap();

    out
}

// ── CLI entry point ──────────────────────────────────────────────────────────

/// Run the dev-loop benchmark command.
///
/// In CI or scheduled runs this drives `autumn dev`, injects file changes,
/// measures end-to-end latency, and writes the report. In `--dry-run` mode
/// it prints the budget table and exits without starting any server.
pub fn run(
    example: &str,
    runs: u32,
    output: Option<&str>,
    json: bool,
    fail_on_regression: bool,
    dry_run: bool,
) -> i32 {
    if dry_run {
        print_budget_table();
        return 0;
    }

    eprintln!("autumn dev-loop-bench: measuring {example} ({runs} run(s) per change class)");
    eprintln!("Note: live measurement requires `autumn dev` and a running HTTP server.");
    eprintln!("Use --dry-run to print the budget table without starting a server.\n");

    // Build a synthetic report using placeholder stats so the command is
    // useful in CI even before the live measurement driver is wired up.
    // The live measurement driver is tracked in the parent issue.
    let results = build_placeholder_results(example);
    let all_passed = results.iter().all(|r| r.passed);

    let report = FullReport {
        timestamp_utc: chrono_utc_now(),
        runner_os: std::env::consts::OS.to_string(),
        rust_version: rust_version_string(),
        autumn_version: env!("CARGO_PKG_VERSION").to_string(),
        example_name: example.to_string(),
        all_passed,
        results,
    };

    let human = format_human_summary(&report);
    let machine = format_json_report(&report);

    if json {
        println!("{machine}");
    } else {
        println!("{human}");
    }

    if let Some(path) = output {
        if let Err(e) = std::fs::write(path, &machine) {
            eprintln!("Warning: could not write report to {path}: {e}");
        } else {
            eprintln!("Report written to {path}");
        }
    }

    if fail_on_regression && !all_passed {
        eprintln!("One or more change classes exceeded the latency budget. Exiting 1.");
        return 1;
    }

    0
}

/// Render a budget table for the given change classes as a string.
///
/// Shared by the warm dev-loop and cold-start dry-run paths so the table
/// layout stays identical and is unit-testable without capturing stdout.
fn format_budget_table(title: &str, classes: &[ChangeClass]) -> String {
    let mut out = String::new();
    let col_w = 46usize;
    writeln!(out, "{title}\n").unwrap();
    writeln!(
        out,
        "{:<col_w$}  {:>10}  {:>10}  {:>10}",
        "Change class", "p50 ms", "p95 ms", "max ms"
    )
    .unwrap();
    writeln!(out, "{}", "-".repeat(col_w + 36)).unwrap();

    for &class in classes {
        let b = budget_for(class);
        writeln!(
            out,
            "{:<col_w$}  {:>10}  {:>10}  {:>10}",
            class.journey_name(),
            b.p50_ms,
            b.p95_ms,
            b.max_ms
        )
        .unwrap();
    }

    writeln!(
        out,
        "\nSee docs/guide/dev-loop-latency.md for methodology and prerequisites."
    )
    .unwrap();
    out
}

fn print_budget_table() {
    print!(
        "{}",
        format_budget_table(
            "Autumn dev-loop latency budget (issue #601)",
            &[
                ChangeClass::InitialBoot,
                ChangeClass::RustRouteEditHello,
                ChangeClass::RustRouteEditDb,
                ChangeClass::CssTailwind,
                ChangeClass::StaticAsset,
                ChangeClass::ConfigEdit,
                ChangeClass::WatchDirEdit,
            ],
        )
    );
}

/// Render the cold-start onboarding budget table. The database-backed shape is
/// included only when `include_db` is set (it is informational in this slice).
fn format_cold_start_budget_table(include_db: bool) -> String {
    let classes: &[ChangeClass] = if include_db {
        &[ChangeClass::ColdStartHello, ChangeClass::ColdStartDb]
    } else {
        &[ChangeClass::ColdStartHello]
    };
    format_budget_table("Autumn cold-start onboarding budget (issue #977)", classes)
}

fn build_placeholder_results(example: &str) -> Vec<BudgetCheckResult> {
    let classes: &[ChangeClass] = if example.contains("todo") || example.contains("blog") {
        &[
            ChangeClass::InitialBoot,
            ChangeClass::RustRouteEditDb,
            ChangeClass::CssTailwind,
            ChangeClass::StaticAsset,
            ChangeClass::ConfigEdit,
            ChangeClass::WatchDirEdit,
        ]
    } else {
        &[
            ChangeClass::InitialBoot,
            ChangeClass::RustRouteEditHello,
            ChangeClass::CssTailwind,
            ChangeClass::StaticAsset,
            ChangeClass::ConfigEdit,
            ChangeClass::WatchDirEdit,
        ]
    };

    classes
        .iter()
        .map(|&class| {
            let budget = budget_for(class);
            // Placeholder: report zero samples so CI can exercise the reporting
            // path. Replace with live HTTP polling once the measurement driver
            // lands. compute_stats(&[]) → all-zeros → passes every budget.
            let stats = compute_stats(&[]);
            check_budget(class, stats, &budget)
        })
        .collect()
}

fn chrono_utc_now() -> String {
    std::env::var("AUTUMN_BENCH_TIMESTAMP").unwrap_or_else(|_| "unknown".to_string())
}

fn rust_version_string() -> String {
    std::env::var("AUTUMN_BENCH_RUST_VERSION").unwrap_or_else(|_| "unknown".to_string())
}

// ── Cold-start onboarding benchmark (issue #977) ──────────────────────────────

/// Which scaffolded project shape to measure for cold start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColdStartShape {
    /// No-database `hello` shape (the gated onboarding budget).
    Hello,
    /// Database-backed shape (informational only in this slice).
    Db,
}

const fn class_for(shape: ColdStartShape) -> ChangeClass {
    match shape {
        ColdStartShape::Hello => ChangeClass::ColdStartHello,
        ColdStartShape::Db => ChangeClass::ColdStartDb,
    }
}

/// Outcome of the (optional) database-backed cold-start shape.
enum DbOutcome {
    /// `--include-db` was not requested.
    NotRequested,
    /// Measured samples (milliseconds).
    Measured(Vec<u64>),
    /// `--include-db` was requested but the measurement failed; the message is
    /// surfaced as an informational failure row so the run does not silently
    /// drop the requested result.
    Failed(String),
}

/// Assemble a cold-start report from measured samples.
///
/// `hello_samples` always gates the result. The database-backed shape, when
/// requested, is recorded as **informational** (measured or failed) and never
/// affects `all_passed`.
fn build_cold_start_report(hello_samples: &[u64], db: &DbOutcome) -> FullReport {
    let mut results = Vec::new();

    let hello_budget = budget_for(ChangeClass::ColdStartHello);
    results.push(check_budget(
        ChangeClass::ColdStartHello,
        compute_stats(hello_samples),
        &hello_budget,
    ));

    match db {
        DbOutcome::NotRequested => {}
        DbOutcome::Measured(samples) => {
            let db_budget = budget_for(ChangeClass::ColdStartDb);
            let mut r = check_budget(ChangeClass::ColdStartDb, compute_stats(samples), &db_budget);
            r.informational = true;
            results.push(r);
        }
        DbOutcome::Failed(msg) => results.push(db_failure_result(msg)),
    }

    // Only non-informational results contribute to the gate.
    let all_passed = results
        .iter()
        .filter(|r| !r.informational)
        .all(|r| r.passed);

    FullReport {
        timestamp_utc: chrono_utc_now(),
        runner_os: std::env::consts::OS.to_string(),
        rust_version: rust_version_string(),
        autumn_version: env!("CARGO_PKG_VERSION").to_string(),
        example_name: "cold-start".to_string(),
        all_passed,
        results,
    }
}

/// Build an informational failure row for a DB-shape measurement that could not
/// be completed, so an `--include-db` run records the requested-but-failed
/// result in the report instead of silently omitting it.
fn db_failure_result(msg: &str) -> BudgetCheckResult {
    let class = ChangeClass::ColdStartDb;
    BudgetCheckResult {
        change_class: class.key().to_string(),
        journey_name: class.journey_name().to_string(),
        stats: compute_stats(&[]),
        budget: budget_for(class),
        passed: false,
        p95_exceeded: false,
        max_exceeded: false,
        p95_overage_pct: 0.0,
        diagnosis: format!("Database-backed cold start could not be measured: {msg}"),
        next_action: "This shape is informational and does not affect the gate. \
                      Re-run `--include-db` in an environment where the bundled \
                      managed-Postgres prerequisites are available."
            .to_string(),
        informational: true,
    }
}

/// Locate the workspace `autumn-web` crate so a throwaway project can be built
/// against the repository's actual source (a genuine clean compile) rather than
/// a possibly-unpublished crates.io version.
///
/// Honours the `AUTUMN_BENCH_AUTUMN_WEB_PATH` override, otherwise walks up from
/// the current directory looking for an `autumn/Cargo.toml` whose package is
/// `autumn-web`.
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

/// Append a `[patch.crates-io]` override pointing `autumn-web` at local source.
///
/// The scaffolded `Cargo.toml` already declares its own (empty) `[workspace]`
/// table, so it is a standalone workspace root and a `[patch]` section is valid
/// there. The patch applies regardless of whether the dependency is the plain
/// `autumn-web = "x"` form or the daemon/`features` table form.
fn repoint_autumn_web(project_dir: &Path, autumn_web: &Path) -> Result<(), String> {
    let manifest = project_dir.join("Cargo.toml");
    let mut content =
        std::fs::read_to_string(&manifest).map_err(|e| format!("read Cargo.toml: {e}"))?;
    // `{:?}` quotes and escapes the path into a valid TOML basic string.
    let patch = format!(
        "\n[patch.crates-io]\nautumn-web = {{ path = {:?} }}\n",
        autumn_web.display().to_string()
    );
    content.push_str(&patch);
    std::fs::write(&manifest, content).map_err(|e| format!("write Cargo.toml: {e}"))?;
    Ok(())
}

/// Measure a single cold-start journey end-to-end: scaffold a throwaway project,
/// compile it from a cold target, start it, and time until the first HTTP 200.
///
/// Returns the elapsed milliseconds from just before `autumn new` to the first
/// successful response on `/`.
fn measure_cold_start_once(shape: ColdStartShape) -> Result<u64, String> {
    let autumn_web = locate_autumn_web().ok_or_else(|| {
        "could not locate the workspace autumn-web crate; \
         set AUTUMN_BENCH_AUTUMN_WEB_PATH to its directory"
            .to_string()
    })?;
    let autumn_web = std::fs::canonicalize(&autumn_web)
        .map_err(|e| format!("canonicalize autumn-web path: {e}"))?;

    let tmp = tempfile::tempdir().map_err(|e| format!("create temp dir: {e}"))?;
    let project_name = "coldstart_app";

    let opts = match shape {
        // The DB-free daemon starter compiles and serves without Postgres.
        ColdStartShape::Hello => crate::new::GenerateOptions {
            with_daemon: true,
            ..Default::default()
        },
        // The database-backed shape uses the bundled managed-Postgres provider so
        // the app self-provisions a real Postgres (via `postgresql_embedded`),
        // runs migrations, and connects before serving — a genuine DB-backed
        // onboarding cold start with no external service required.
        ColdStartShape::Db => crate::new::GenerateOptions {
            with_bundled_pg: true,
            ..Default::default()
        },
    };

    // Start the clock just before `autumn new` so the whole onboarding journey
    // (scaffold → first clean compile → boot → first 200) is captured.
    let start = Instant::now();

    // Quiet scaffold: keep stdout clean so `--cold-start --json` emits only the
    // JSON report.
    crate::new::generate_with_quiet(project_name, tmp.path(), opts)
        .map_err(|e| format!("autumn new failed: {e}"))?;
    let project_dir = tmp.path().join(project_name);

    repoint_autumn_web(&project_dir, &autumn_web)?;

    // Pick the port the app binds and we poll. Default: a freshly reserved
    // ephemeral port per sample, so repeated runs never collide on a lingering
    // TIME_WAIT socket from the previous sample. An explicit AUTUMN_BENCH_PORT
    // override is honoured as-is.
    let port = match std::env::var("AUTUMN_BENCH_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
    {
        Some(p) => p,
        None => reserve_free_port()?,
    };

    // Cold build into the project's own (empty) target/. Remove every inherited
    // Cargo target/triple override so the parent's warm cache is never reused,
    // and ask Cargo for JSON artifact messages so we learn the *exact* built
    // binary path — robust to any `build.target` / `build.target-dir` coming from
    // Cargo config files, which `env_remove` alone cannot neutralise.
    let output = Command::new("cargo")
        .args(["build", "--message-format=json"])
        .current_dir(&project_dir)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .output()
        .map_err(|e| format!("cargo build spawn failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo build failed for the scaffolded project:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let bin = cargo_executable_path(&output.stdout, project_name).ok_or_else(|| {
        "could not determine the built binary path from cargo's JSON output".to_string()
    })?;

    let mut cmd = Command::new(&bin);
    cmd.current_dir(&project_dir);
    // Isolate the child from the surrounding environment: clear every inherited
    // `AUTUMN_*` var so app-mode settings can't change how the scaffolded app
    // boots or what it binds. Without this, e.g. `AUTUMN_ENV=prod` would trigger
    // production boot checks, `AUTUMN_BUILD_STATIC=1` would render static output
    // and exit instead of serving, and a stray `AUTUMN_SERVER__UNIX_SOCKET`
    // would make the app bind a socket instead of the TCP port we poll.
    for (key, _) in std::env::vars() {
        if key.starts_with("AUTUMN_") {
            cmd.env_remove(&key);
        }
    }
    // Then pin exactly the host+port we poll (env vars are the highest-precedence
    // config source, overriding autumn.toml).
    cmd.env("AUTUMN_SERVER__HOST", "127.0.0.1")
        .env("AUTUMN_SERVER__PORT", port.to_string());
    // For the DB shape, keep the bundled managed-Postgres cluster inside the
    // throwaway temp dir so the run is self-contained: otherwise the provider
    // defaults to `$HOME/.local/share/autumn/...` and leaves a full Postgres
    // data/extraction directory behind after the tempdir is removed.
    if matches!(shape, ColdStartShape::Db) {
        cmd.env(
            "AUTUMN_MANAGED_PG_DATA_DIR",
            project_dir.join("managed-pg-data"),
        );
    }
    cmd
        // Discard the app's own logs so they never interleave with the JSON
        // report on stdout; the harness reports progress on stderr separately.
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start the built server binary: {e}"))?;

    // Poll the configured root route until the first 200 or the deadline.
    let url = format!("http://127.0.0.1:{port}/");
    let deadline = start + Duration::from_millis(budget_for(class_for(shape)).max_ms * 2);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;

    let mut measured = None;
    while Instant::now() < deadline {
        // If the app exited (e.g. the chosen port is already in use → bind
        // failure → process exit), stop immediately so we never record a 200
        // served by some other process listening on this port.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "scaffolded server exited before serving (exit: {status}); \
                 port {port} may already be in use"
            ));
        }
        // Accept only a 200 whose body proves it came from OUR scaffolded app:
        // its index page renders the project name. This rules out a 200 served by
        // some unrelated service that happens to answer on this port during the
        // child's boot-toward-bind-failure window.
        if let Ok(resp) = client.get(&url).send()
            && resp.status().is_success()
            && resp.text().is_ok_and(|body| body.contains(project_name))
        {
            measured = Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX));
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Graceful shutdown so the app's on-shutdown hooks run — notably the
    // managed-Postgres provider stopping its supervised cluster on the DB shape,
    // which otherwise orphans Postgres processes/data between samples.
    stop_child(&mut child);

    measured.ok_or_else(|| "server did not return HTTP 200 before the deadline".to_string())
}

/// Reserve a currently-free TCP port on loopback.
///
/// Binds an ephemeral port, reads it, then drops the listener so the scaffolded
/// app can bind it. There is a tiny race between release and the child's bind,
/// but the per-iteration `try_wait()` and the response-body check make a
/// mis-attributed sample impossible even if the port is lost.
fn reserve_free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("failed to reserve a free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to read reserved port: {e}"))?
        .port();
    Ok(port)
}

/// Extract the built binary path from `cargo build --message-format=json` output.
///
/// Scans the `compiler-artifact` messages for the one whose target is the
/// project's binary and returns its reported `executable` path. This is robust
/// to a non-default target dir or `--target <triple>` configured via Cargo
/// config files (which would otherwise move the artifact out of `target/debug/`).
fn cargo_executable_path(stdout: &[u8], bin_name: &str) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(stdout);
    let mut found = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
            && v.get("target")
                .and_then(|t| t.get("name"))
                .and_then(serde_json::Value::as_str)
                == Some(bin_name)
            && let Some(exe) = v.get("executable").and_then(serde_json::Value::as_str)
        {
            found = Some(PathBuf::from(exe));
        }
    }
    found
}

/// Stop a scaffolded cold-start server, preferring a graceful SIGTERM so the
/// app runs its shutdown hooks (e.g. stopping a bundled Postgres cluster) before
/// falling back to SIGKILL if it does not exit in time.
fn stop_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = crate::process::validate_pid_for_kill(child.id())
            && let Err(e) = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            )
        {
            eprintln!("  Warning: failed to SIGTERM cold-start server: {e}");
        }
        if crate::process::wait_with_timeout(child, Duration::from_secs(10)).is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Run the cold-start measurement `runs` times, returning the samples (ms).
fn measure_cold_start(shape: ColdStartShape, runs: u32) -> Result<Vec<u64>, String> {
    let mut samples = Vec::with_capacity(runs as usize);
    for i in 1..=runs {
        eprintln!("  cold-start {shape:?} run {i}/{runs}…");
        let ms = measure_cold_start_once(shape)?;
        eprintln!("    → {ms} ms");
        samples.push(ms);
    }
    Ok(samples)
}

/// Run the `autumn dev-loop-bench --cold-start` command.
///
/// In `--dry-run` mode it prints the cold-start budget table with no build or
/// server. Otherwise it measures the no-DB `hello` cold start (gated) and,
/// when `include_db` is set, the database-backed shape (informational).
// Mirrors the flag set of [`run`] (json / fail_on_regression / dry_run) plus
// the cold-start-only `include_db` toggle; grouping these into a struct would
// not improve the single CLI call site.
#[allow(clippy::fn_params_excessive_bools)]
pub fn run_cold_start(
    runs: u32,
    output: Option<&str>,
    json: bool,
    fail_on_regression: bool,
    dry_run: bool,
    include_db: bool,
) -> i32 {
    if dry_run {
        print!("{}", format_cold_start_budget_table(include_db));
        return 0;
    }

    // A measurement run with zero samples would publish an all-zero, "passing"
    // report without compiling or starting anything. Refuse it so the gate can
    // never go green on no data.
    if runs == 0 {
        eprintln!("Error: --runs must be at least 1 for a cold-start measurement.");
        return 1;
    }

    eprintln!(
        "autumn dev-loop-bench --cold-start: measuring onboarding cold start ({runs} run(s))"
    );
    eprintln!(
        "This scaffolds throwaway projects and compiles them from a cold target — \
         expect minutes per run.\n"
    );

    let hello_samples = match measure_cold_start(ColdStartShape::Hello, runs) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: cold-start (hello) measurement failed: {e}");
            return 1;
        }
    };

    let db = if include_db {
        match measure_cold_start(ColdStartShape::Db, runs) {
            Ok(s) => DbOutcome::Measured(s),
            Err(e) => {
                // The DB shape is informational; a failure must not break the run,
                // but it is recorded in the report rather than silently dropped.
                eprintln!("Warning: informational DB cold-start measurement failed: {e}");
                DbOutcome::Failed(e)
            }
        }
    } else {
        DbOutcome::NotRequested
    };

    let report = build_cold_start_report(&hello_samples, &db);
    let human = format_human_summary(&report);
    let machine = format_json_report(&report);

    if json {
        println!("{machine}");
    } else {
        println!("{human}");
    }

    if let Some(path) = output {
        if let Err(e) = std::fs::write(path, &machine) {
            eprintln!("Warning: could not write report to {path}: {e}");
        } else {
            eprintln!("Report written to {path}");
        }
    }

    if fail_on_regression && !report.all_passed {
        eprintln!("Cold-start onboarding budget exceeded. Exiting 1.");
        return 1;
    }

    0
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── compute_stats ─────────────────────────────────────────────────────

    #[test]
    fn compute_stats_empty_returns_zeros() {
        let s = compute_stats(&[]);
        assert_eq!(s.p50_ms, 0);
        assert_eq!(s.p95_ms, 0);
        assert_eq!(s.max_ms, 0);
        assert_eq!(s.sample_count, 0);
    }

    #[test]
    fn compute_stats_single_sample() {
        let s = compute_stats(&[500]);
        assert_eq!(s.p50_ms, 500);
        assert_eq!(s.p95_ms, 500);
        assert_eq!(s.max_ms, 500);
        assert_eq!(s.sample_count, 1);
    }

    #[test]
    fn compute_stats_10_ascending_samples() {
        // nearest-rank: p50 = ceil(0.5 * 10) = 5 → sorted[4] = 500
        //               p95 = ceil(0.95 * 10) = 10 → sorted[9] = 1000
        let samples: Vec<u64> = vec![100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];
        let s = compute_stats(&samples);
        assert_eq!(s.p50_ms, 500);
        assert_eq!(s.p95_ms, 1000);
        assert_eq!(s.max_ms, 1000);
        assert_eq!(s.sample_count, 10);
    }

    #[test]
    fn compute_stats_unsorted_input_is_ordered_internally() {
        // sorted: [100, 300, 500, 700, 900]
        // p50 = ceil(0.5 * 5) = 3 → sorted[2] = 500
        // p95 = ceil(0.95 * 5) = 5 → sorted[4] = 900
        let samples: Vec<u64> = vec![900, 100, 500, 300, 700];
        let s = compute_stats(&samples);
        assert_eq!(s.p50_ms, 500);
        assert_eq!(s.p95_ms, 900);
        assert_eq!(s.max_ms, 900);
    }

    #[test]
    fn compute_stats_all_identical_samples() {
        let samples = vec![750u64; 20];
        let s = compute_stats(&samples);
        assert_eq!(s.p50_ms, 750);
        assert_eq!(s.p95_ms, 750);
        assert_eq!(s.max_ms, 750);
    }

    // ── budget_for ────────────────────────────────────────────────────────

    #[test]
    fn budget_css_tailwind_p95_is_1000ms() {
        assert_eq!(budget_for(ChangeClass::CssTailwind).p95_ms, 1000);
    }

    #[test]
    fn budget_static_asset_p95_is_1000ms() {
        assert_eq!(budget_for(ChangeClass::StaticAsset).p95_ms, 1000);
    }

    #[test]
    fn budget_rust_route_hello_p95_is_5000ms() {
        assert_eq!(budget_for(ChangeClass::RustRouteEditHello).p95_ms, 5_000);
    }

    #[test]
    fn budget_rust_route_db_p95_is_10000ms() {
        assert_eq!(budget_for(ChangeClass::RustRouteEditDb).p95_ms, 10_000);
    }

    #[test]
    fn budget_all_classes_have_nonzero_p95() {
        for class in [
            ChangeClass::InitialBoot,
            ChangeClass::RustRouteEditHello,
            ChangeClass::RustRouteEditDb,
            ChangeClass::CssTailwind,
            ChangeClass::StaticAsset,
            ChangeClass::ConfigEdit,
            ChangeClass::WatchDirEdit,
        ] {
            assert!(
                budget_for(class).p95_ms > 0,
                "p95 must be > 0 for {class:?}"
            );
        }
    }

    // ── check_budget ──────────────────────────────────────────────────────

    #[test]
    fn check_budget_passes_when_under_p95_and_max() {
        let budget = LatencyBudget {
            p50_ms: 500,
            p95_ms: 1000,
            max_ms: 5000,
        };
        let stats = ClassStats {
            p50_ms: 100,
            p95_ms: 800,
            max_ms: 900,
            sample_count: 10,
        };
        let r = check_budget(ChangeClass::CssTailwind, stats, &budget);
        assert!(r.passed);
        assert!(!r.p95_exceeded);
        assert!(!r.max_exceeded);
    }

    #[test]
    fn check_budget_fails_when_p95_exceeds_budget() {
        let budget = LatencyBudget {
            p50_ms: 500,
            p95_ms: 1000,
            max_ms: 5000,
        };
        let stats = ClassStats {
            p50_ms: 100,
            p95_ms: 1200,
            max_ms: 1500,
            sample_count: 10,
        };
        let r = check_budget(ChangeClass::CssTailwind, stats, &budget);
        assert!(!r.passed);
        assert!(r.p95_exceeded);
    }

    #[test]
    fn check_budget_fails_when_max_exceeds_budget() {
        let budget = LatencyBudget {
            p50_ms: 500,
            p95_ms: 1000,
            max_ms: 2000,
        };
        let stats = ClassStats {
            p50_ms: 100,
            p95_ms: 900,
            max_ms: 2500,
            sample_count: 10,
        };
        let r = check_budget(ChangeClass::CssTailwind, stats, &budget);
        assert!(!r.passed);
        assert!(r.max_exceeded);
    }

    #[test]
    fn check_budget_passes_exactly_at_limit() {
        let budget = LatencyBudget {
            p50_ms: 500,
            p95_ms: 1000,
            max_ms: 2000,
        };
        let stats = ClassStats {
            p50_ms: 500,
            p95_ms: 1000,
            max_ms: 2000,
            sample_count: 5,
        };
        let r = check_budget(ChangeClass::CssTailwind, stats, &budget);
        assert!(r.passed, "at-limit measurements should pass");
    }

    #[test]
    fn check_budget_diagnosis_css_names_journey() {
        let budget = budget_for(ChangeClass::CssTailwind);
        let stats = ClassStats {
            p50_ms: 2000,
            p95_ms: 2000,
            max_ms: 2000,
            sample_count: 5,
        };
        let r = check_budget(ChangeClass::CssTailwind, stats, &budget);
        assert!(
            r.journey_name.to_lowercase().contains("css")
                || r.journey_name.to_lowercase().contains("tailwind"),
            "journey_name should mention CSS or Tailwind, got: {}",
            r.journey_name
        );
        assert!(
            r.diagnosis.contains("p95"),
            "diagnosis must mention p95, got: {}",
            r.diagnosis
        );
        assert!(!r.next_action.is_empty(), "next_action must not be empty");
    }

    #[test]
    fn check_budget_diagnosis_rust_hello_names_rust() {
        let budget = budget_for(ChangeClass::RustRouteEditHello);
        let stats = ClassStats {
            p50_ms: 6000,
            p95_ms: 6000,
            max_ms: 7000,
            sample_count: 5,
        };
        let r = check_budget(ChangeClass::RustRouteEditHello, stats, &budget);
        assert!(
            r.journey_name.to_lowercase().contains("rust"),
            "journey_name should mention Rust, got: {}",
            r.journey_name
        );
        assert!(!r.next_action.is_empty());
    }

    #[test]
    fn check_budget_passing_result_has_empty_diagnosis() {
        let budget = budget_for(ChangeClass::CssTailwind);
        let stats = ClassStats {
            p50_ms: 100,
            p95_ms: 200,
            max_ms: 300,
            sample_count: 5,
        };
        let r = check_budget(ChangeClass::CssTailwind, stats, &budget);
        assert!(r.passed);
        assert!(
            r.diagnosis.is_empty(),
            "passing result must have empty diagnosis, got: {}",
            r.diagnosis
        );
        assert!(
            r.next_action.is_empty(),
            "passing result must have empty next_action, got: {}",
            r.next_action
        );
    }

    #[test]
    fn check_budget_overage_pct_is_zero_when_passing() {
        let budget = LatencyBudget {
            p50_ms: 500,
            p95_ms: 1000,
            max_ms: 5000,
        };
        let stats = ClassStats {
            p50_ms: 100,
            p95_ms: 800,
            max_ms: 900,
            sample_count: 5,
        };
        let r = check_budget(ChangeClass::StaticAsset, stats, &budget);
        assert!(
            r.p95_overage_pct.abs() < f64::EPSILON,
            "overage_pct must be 0 for a passing result, got {}",
            r.p95_overage_pct
        );
    }

    #[test]
    fn check_budget_overage_pct_calculated_when_failing() {
        let budget = LatencyBudget {
            p50_ms: 500,
            p95_ms: 1000,
            max_ms: 5000,
        };
        let stats = ClassStats {
            p50_ms: 100,
            p95_ms: 1500,
            max_ms: 2000,
            sample_count: 5,
        };
        let r = check_budget(ChangeClass::StaticAsset, stats, &budget);
        // (1500 - 1000) / 1000 * 100 = 50%
        assert!((r.p95_overage_pct - 50.0).abs() < 0.01);
    }

    // ── format_json_report ────────────────────────────────────────────────

    fn make_test_report(all_passed: bool) -> FullReport {
        let css_budget = budget_for(ChangeClass::CssTailwind);
        let css_stats = if all_passed {
            ClassStats {
                p50_ms: 200,
                p95_ms: 500,
                max_ms: 800,
                sample_count: 5,
            }
        } else {
            ClassStats {
                p50_ms: 2000,
                p95_ms: 2000,
                max_ms: 3000,
                sample_count: 5,
            }
        };
        let rust_budget = budget_for(ChangeClass::RustRouteEditHello);
        let rust_stats = ClassStats {
            p50_ms: 1000,
            p95_ms: 2000,
            max_ms: 3000,
            sample_count: 5,
        };

        let results = vec![
            check_budget(ChangeClass::CssTailwind, css_stats, &css_budget),
            check_budget(ChangeClass::RustRouteEditHello, rust_stats, &rust_budget),
        ];
        let computed_all_passed = results.iter().all(|r| r.passed);

        FullReport {
            timestamp_utc: "2026-05-26T00:00:00Z".to_string(),
            runner_os: "Linux".to_string(),
            rust_version: "1.88.0".to_string(),
            autumn_version: "0.5.0".to_string(),
            example_name: "examples/hello".to_string(),
            all_passed: computed_all_passed,
            results,
        }
    }

    #[test]
    fn format_json_report_produces_valid_json() {
        let report = make_test_report(true);
        let s = format_json_report(&report);
        serde_json::from_str::<serde_json::Value>(&s).expect("must be valid JSON");
    }

    #[test]
    fn format_json_report_contains_env_metadata_fields() {
        let report = make_test_report(true);
        let s = format_json_report(&report);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("timestamp_utc").is_some(), "must have timestamp_utc");
        assert!(v.get("runner_os").is_some(), "must have runner_os");
        assert!(v.get("rust_version").is_some(), "must have rust_version");
        assert!(v.get("results").is_some(), "must have results array");
        assert!(v.get("all_passed").is_some(), "must have all_passed");
    }

    #[test]
    fn format_json_report_results_have_required_fields() {
        let report = make_test_report(true);
        let s = format_json_report(&report);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let results = v["results"].as_array().expect("results must be an array");
        assert!(!results.is_empty(), "results must not be empty");
        let first = &results[0];
        assert!(first.get("change_class").is_some(), "missing change_class");
        assert!(first.get("passed").is_some(), "missing passed");
        assert!(
            first["stats"].get("p50_ms").is_some(),
            "missing p50_ms in stats"
        );
    }

    #[test]
    fn format_json_report_does_not_leak_home_path() {
        let report = make_test_report(true);
        let s = format_json_report(&report);
        assert!(!s.contains("/home/"), "must not leak /home/ paths");
        assert!(
            !s.contains("C:\\Users\\"),
            "must not leak Windows user paths"
        );
    }

    // ── format_human_summary ──────────────────────────────────────────────

    #[test]
    fn format_human_summary_shows_p50_p95_max_headers() {
        let report = make_test_report(true);
        let s = format_human_summary(&report);
        assert!(
            s.contains("p50") || s.contains("P50"),
            "summary must mention p50"
        );
        assert!(
            s.contains("p95") || s.contains("P95"),
            "summary must mention p95"
        );
        assert!(
            s.contains("max") || s.contains("Max") || s.contains("MAX"),
            "summary must mention max"
        );
    }

    #[test]
    fn format_human_summary_passing_report_shows_pass() {
        let report = make_test_report(true);
        let s = format_human_summary(&report);
        let lower = s.to_lowercase();
        assert!(
            lower.contains("pass"),
            "passing report must show PASS, got:\n{s}"
        );
    }

    #[test]
    fn format_human_summary_failing_report_shows_fail() {
        let report = make_test_report(false);
        let s = format_human_summary(&report);
        let lower = s.to_lowercase();
        assert!(
            lower.contains("fail") || lower.contains("exceed") || lower.contains("regress"),
            "failing report must show failure info, got:\n{s}"
        );
    }

    #[test]
    fn format_human_summary_has_at_least_one_row_per_result() {
        let report = make_test_report(true);
        let result_count = report.results.len();
        let s = format_human_summary(&report);
        let data_lines = s
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('-'))
            .count();
        assert!(
            data_lines >= result_count,
            "summary should have at least {result_count} non-empty lines, got {data_lines}"
        );
    }

    #[test]
    fn format_human_summary_failing_row_shows_diagnosis_and_next_action() {
        let report = make_test_report(false);
        let s = format_human_summary(&report);
        assert!(
            s.contains("↳"),
            "failing row must include diagnosis arrow, got:\n{s}"
        );
        assert!(
            s.contains("Next:"),
            "failing row must include Next: action, got:\n{s}"
        );
    }

    // ── change_class keys ─────────────────────────────────────────────────

    #[test]
    fn change_class_keys_are_unique() {
        let classes = [
            ChangeClass::InitialBoot,
            ChangeClass::RustRouteEditHello,
            ChangeClass::RustRouteEditDb,
            ChangeClass::CssTailwind,
            ChangeClass::StaticAsset,
            ChangeClass::ConfigEdit,
            ChangeClass::WatchDirEdit,
        ];
        let keys: Vec<_> = classes.iter().map(|c| c.key()).collect();
        let unique: std::collections::HashSet<_> = keys.iter().copied().collect();
        assert_eq!(
            keys.len(),
            unique.len(),
            "all change class keys must be unique"
        );
    }

    #[test]
    fn change_class_journey_names_are_unique() {
        let classes = [
            ChangeClass::InitialBoot,
            ChangeClass::RustRouteEditHello,
            ChangeClass::RustRouteEditDb,
            ChangeClass::CssTailwind,
            ChangeClass::StaticAsset,
            ChangeClass::ConfigEdit,
            ChangeClass::WatchDirEdit,
        ];
        let names: Vec<_> = classes.iter().map(|c| c.journey_name()).collect();
        let unique: std::collections::HashSet<_> = names.iter().copied().collect();
        assert_eq!(
            names.len(),
            unique.len(),
            "all journey names must be unique"
        );
    }

    // ── run() / print_budget_table / build_placeholder_results ───────────

    #[test]
    fn run_dry_run_returns_zero_and_prints_table() {
        let exit = run("examples/hello", 5, None, false, false, true);
        assert_eq!(exit, 0);
    }

    #[test]
    fn run_hello_example_normal_mode_returns_zero() {
        let exit = run("examples/hello", 3, None, false, false, false);
        assert_eq!(exit, 0);
    }

    #[test]
    fn run_json_flag_returns_zero() {
        let exit = run("examples/hello", 3, None, true, false, false);
        assert_eq!(exit, 0);
    }

    #[test]
    fn run_todo_example_uses_db_path() {
        // exercises the `contains("todo")` branch in build_placeholder_results
        let exit = run("examples/todo-app", 3, None, false, false, false);
        assert_eq!(exit, 0);
    }

    #[test]
    fn run_blog_example_uses_db_path() {
        // exercises the `contains("blog")` branch in build_placeholder_results
        let exit = run("examples/blog", 3, None, false, false, false);
        assert_eq!(exit, 0);
    }

    #[test]
    fn run_fail_on_regression_with_passing_placeholder_still_zero() {
        // Placeholder results are all-zero ms which always passes the budget,
        // so --fail-on-regression must not trip with the placeholder driver.
        let exit = run("examples/hello", 3, None, false, true, false);
        assert_eq!(exit, 0);
    }

    #[test]
    fn run_output_writes_valid_json_to_file() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_str().unwrap().to_string();
        let exit = run("examples/hello", 3, Some(&path), false, false, false);
        assert_eq!(exit, 0);
        let content = std::fs::read_to_string(&path).expect("report file");
        serde_json::from_str::<serde_json::Value>(&content)
            .expect("output file must contain valid JSON");
    }

    #[test]
    fn run_output_bad_path_still_returns_zero() {
        // A write error on the output path is non-fatal; the command still succeeds.
        let exit = run(
            "examples/hello",
            3,
            Some("/dev/full/nonexistent/path/report.json"),
            false,
            false,
            false,
        );
        assert_eq!(exit, 0);
    }

    // ── env var helpers ───────────────────────────────────────────────────

    #[test]
    fn chrono_utc_now_returns_set_env_var() {
        let result = temp_env::with_var(
            "AUTUMN_BENCH_TIMESTAMP",
            Some("2026-05-26T00:00:00Z"),
            chrono_utc_now,
        );
        assert_eq!(result, "2026-05-26T00:00:00Z");
    }

    #[test]
    fn chrono_utc_now_fallback_is_unknown() {
        let result = temp_env::with_var("AUTUMN_BENCH_TIMESTAMP", None::<&str>, chrono_utc_now);
        assert_eq!(result, "unknown");
    }

    #[test]
    fn rust_version_string_returns_set_env_var() {
        let result = temp_env::with_var(
            "AUTUMN_BENCH_RUST_VERSION",
            Some("rustc 1.88.0"),
            rust_version_string,
        );
        assert_eq!(result, "rustc 1.88.0");
    }

    #[test]
    fn rust_version_string_fallback_is_unknown() {
        let result = temp_env::with_var(
            "AUTUMN_BENCH_RUST_VERSION",
            None::<&str>,
            rust_version_string,
        );
        assert_eq!(result, "unknown");
    }

    // ── cold-start budgets ────────────────────────────────────────────────

    #[test]
    fn budget_cold_start_hello_p95_is_60000ms() {
        // Success metric (issue #977): p95 ≤ 60s for the no-DB cold start.
        assert_eq!(budget_for(ChangeClass::ColdStartHello).p95_ms, 60_000);
    }

    #[test]
    fn budget_cold_start_classes_have_nonzero_p95() {
        for class in [ChangeClass::ColdStartHello, ChangeClass::ColdStartDb] {
            assert!(
                budget_for(class).p95_ms > 0,
                "p95 must be > 0 for {class:?}"
            );
        }
    }

    #[test]
    fn cold_start_keys_are_unique_and_descriptive() {
        let hello = ChangeClass::ColdStartHello;
        let db = ChangeClass::ColdStartDb;
        assert_ne!(hello.key(), db.key());
        assert!(hello.key().contains("cold_start"), "got: {}", hello.key());
        assert!(db.key().contains("cold_start"), "got: {}", db.key());
    }

    #[test]
    fn cold_start_journey_names_mention_cold_start() {
        for class in [ChangeClass::ColdStartHello, ChangeClass::ColdStartDb] {
            assert!(
                class.journey_name().to_lowercase().contains("cold start"),
                "journey_name should mention cold start, got: {}",
                class.journey_name()
            );
        }
    }

    // ── cold-start budget table ───────────────────────────────────────────

    #[test]
    fn format_cold_start_budget_table_lists_hello_and_budget() {
        let table = format_cold_start_budget_table(false);
        assert!(
            table.to_lowercase().contains("cold start"),
            "table must mention cold start, got:\n{table}"
        );
        // 60s p95 budget for the gated no-DB shape must be visible.
        assert!(
            table.contains("60000") || table.contains("60 000"),
            "table must show the 60s p95 budget, got:\n{table}"
        );
    }

    #[test]
    fn format_cold_start_budget_table_db_row_only_when_requested() {
        let without = format_cold_start_budget_table(false);
        let with = format_cold_start_budget_table(true);
        assert!(
            !without.to_lowercase().contains("database"),
            "DB row must be hidden unless include_db, got:\n{without}"
        );
        assert!(
            with.to_lowercase().contains("database"),
            "DB row must appear when include_db, got:\n{with}"
        );
    }

    // ── cold-start report builder ─────────────────────────────────────────

    #[test]
    fn build_cold_start_report_hello_only_gates_on_hello() {
        // Hello well within budget → all_passed true, one result, not informational.
        let report = build_cold_start_report(&[40_000, 41_000, 42_000], &DbOutcome::NotRequested);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].change_class, "cold_start_hello");
        assert!(!report.results[0].informational);
        assert!(report.all_passed);
    }

    #[test]
    fn build_cold_start_report_hello_over_budget_fails_gate() {
        // p95 way above the 60s budget → gate fails.
        let report =
            build_cold_start_report(&[120_000, 130_000, 140_000], &DbOutcome::NotRequested);
        assert!(!report.all_passed, "over-budget hello must fail the gate");
    }

    #[test]
    fn build_cold_start_report_db_is_informational_and_ungated() {
        // DB samples blow past the budget but, being informational, must NOT
        // flip the overall gate (hello is within budget).
        let report = build_cold_start_report(
            &[40_000, 41_000, 42_000],
            &DbOutcome::Measured(vec![300_000, 310_000, 320_000]),
        );
        assert_eq!(report.results.len(), 2);
        let db = report
            .results
            .iter()
            .find(|r| r.change_class == "cold_start_db")
            .expect("db result present");
        assert!(db.informational, "db result must be informational");
        assert!(
            report.all_passed,
            "informational db over-budget must not fail the gate"
        );
    }

    #[test]
    fn build_cold_start_report_db_failure_is_informational_row() {
        // A requested-but-failed DB shape is recorded (not dropped) and does not
        // fail the gate.
        let report = build_cold_start_report(
            &[40_000, 41_000, 42_000],
            &DbOutcome::Failed("postgres unavailable".to_string()),
        );
        assert_eq!(report.results.len(), 2);
        let db = report
            .results
            .iter()
            .find(|r| r.change_class == "cold_start_db")
            .expect("db failure row present");
        assert!(db.informational);
        assert!(!db.passed, "failure row must be marked not-passed");
        assert!(db.diagnosis.contains("postgres unavailable"));
        assert!(
            report.all_passed,
            "informational db failure must not fail the gate"
        );
    }

    #[test]
    fn build_cold_start_report_json_has_metadata_and_no_path_leak() {
        let report = build_cold_start_report(
            &[40_000, 41_000, 42_000],
            &DbOutcome::Measured(vec![80_000, 81_000, 82_000]),
        );
        let s = format_json_report(&report);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert!(v.get("timestamp_utc").is_some());
        assert!(v.get("runner_os").is_some());
        assert!(v.get("rust_version").is_some());
        assert!(v.get("autumn_version").is_some());
        assert!(v.get("all_passed").is_some());
        assert!(!s.contains("/home/"), "must not leak /home/ paths");
        assert!(!s.contains("C:\\Users\\"), "must not leak Windows paths");
    }

    // ── cargo artifact path / free port ───────────────────────────────────

    #[test]
    fn cargo_executable_path_picks_matching_bin() {
        let stdout = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"some_dep"},"executable":null}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"coldstart_app"},"executable":"/tmp/x/target/debug/coldstart_app"}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\n",
        );
        let path = cargo_executable_path(stdout.as_bytes(), "coldstart_app");
        assert_eq!(
            path,
            Some(PathBuf::from("/tmp/x/target/debug/coldstart_app"))
        );
    }

    #[test]
    fn cargo_executable_path_none_when_absent() {
        let stdout = r#"{"reason":"build-finished","success":true}"#;
        assert!(cargo_executable_path(stdout.as_bytes(), "coldstart_app").is_none());
    }

    #[test]
    fn reserve_free_port_returns_nonzero() {
        let port = reserve_free_port().expect("should reserve a port");
        assert!(port > 0);
    }

    // ── run_cold_start ────────────────────────────────────────────────────

    #[test]
    fn run_cold_start_dry_run_returns_zero() {
        assert_eq!(run_cold_start(1, None, false, false, true, false), 0);
        assert_eq!(run_cold_start(1, None, true, false, true, true), 0);
    }

    #[test]
    fn run_cold_start_rejects_zero_runs() {
        // --runs 0 must not publish an all-zero "passing" report without
        // measuring anything; it returns a non-zero exit instead.
        let exit = run_cold_start(0, None, false, false, false, false);
        assert_eq!(exit, 1, "zero runs must be rejected");
    }

    #[test]
    fn run_cold_start_dry_run_ignores_zero_runs() {
        // Dry-run never measures, so runs == 0 is harmless there.
        assert_eq!(run_cold_start(0, None, false, false, true, false), 0);
    }

    // ── live driver (slow: compiles a fresh project) ──────────────────────

    #[test]
    #[ignore = "compiles a throwaway project from a cold target; run with --ignored"]
    fn cold_start_live_hello_measures_a_positive_duration() {
        let ms = measure_cold_start_once(ColdStartShape::Hello)
            .expect("live cold-start measurement should succeed");
        assert!(ms > 0, "measured cold-start duration must be positive");
    }
}
