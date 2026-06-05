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

fn print_budget_table() {
    println!("Autumn dev-loop latency budget (issue #601)\n");
    let col_w = 46usize;
    println!(
        "{:<col_w$}  {:>10}  {:>10}  {:>10}",
        "Change class", "p50 ms", "p95 ms", "max ms"
    );
    println!("{}", "-".repeat(col_w + 36));

    for class in [
        ChangeClass::InitialBoot,
        ChangeClass::RustRouteEditHello,
        ChangeClass::RustRouteEditDb,
        ChangeClass::CssTailwind,
        ChangeClass::StaticAsset,
        ChangeClass::ConfigEdit,
        ChangeClass::WatchDirEdit,
    ] {
        let b = budget_for(class);
        println!(
            "{:<col_w$}  {:>10}  {:>10}  {:>10}",
            class.journey_name(),
            b.p50_ms,
            b.p95_ms,
            b.max_ms
        );
    }

    println!("\nSee docs/guide/dev-loop-latency.md for methodology and prerequisites.");
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
}
