//! `autumn doctor` — first-run environment diagnostics.
//!
//! Runs a set of checks against the local environment and project configuration,
//! reports each as ✅/⚠️/❌ with a one-line remediation hint, and exits with
//! code 0 (all clear) or 1 (any failure detected).

use serde::Serialize;

/// Known top-level keys in a valid `autumn.toml`.
const KNOWN_TOML_SECTIONS: &[&str] = &[
    "server",
    "database",
    "log",
    "telemetry",
    "health",
    "actuator",
    "cors",
    "session",
    "jobs",
    "auth",
    "security",
    "i18n",
    "storage",
    "mail",
    "profile",
];

/// Result status for a single diagnostic check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// Result of a single diagnostic check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    /// Short identifier for this check.
    pub name: &'static str,
    pub status: CheckStatus,
    /// Human-readable detail (what was found, or what went wrong).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// One-line remediation hint shown on warn/fail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<&'static str>,
}

/// Aggregate counts across all checks.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub passed: usize,
    pub warned: usize,
    pub failed: usize,
}

/// Options parsed from CLI flags.
#[derive(Clone, Copy)]
pub struct DoctorOptions {
    /// Emit machine-readable JSON instead of human text.
    pub json: bool,
    /// Treat warnings as failures (exit 1).
    pub strict: bool,
}

/// Extension point: implement this trait to add custom checks.
#[allow(dead_code)]
pub trait Check {
    fn run(&self) -> CheckResult;
}

// ─── Pure helper functions (fully unit-testable) ──────────────────────────────

pub const fn glyph(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Pass => "✅",
        CheckStatus::Warn => "⚠️ ",
        CheckStatus::Fail => "❌",
    }
}

pub fn compute_summary(results: &[CheckResult]) -> Summary {
    let mut passed = 0;
    let mut warned = 0;
    let mut failed = 0;
    for r in results {
        match r.status {
            CheckStatus::Pass => passed += 1,
            CheckStatus::Warn => warned += 1,
            CheckStatus::Fail => failed += 1,
        }
    }
    Summary {
        passed,
        warned,
        failed,
    }
}

pub const fn exit_code(summary: &Summary, strict: bool) -> i32 {
    if summary.failed > 0 || (strict && summary.warned > 0) {
        1
    } else {
        0
    }
}

pub fn format_check_line(result: &CheckResult) -> String {
    use std::fmt::Write as _;
    let g = glyph(&result.status);
    let mut line = format!("{g} {}", result.name);
    if let Some(ref detail) = result.detail {
        let _ = write!(line, " — {detail}");
    }
    if let Some(hint) = result.hint
        && result.status != CheckStatus::Pass
    {
        let _ = write!(line, "\n   hint: {hint}");
    }
    line
}

pub fn format_summary_line(summary: &Summary, code: i32) -> String {
    let verdict = if code == 0 {
        "all clear"
    } else {
        "problems found"
    };
    let w_label = if summary.warned == 1 {
        "warning"
    } else {
        "warnings"
    };
    format!(
        "{} passed, {} {}, {} failed — {verdict}",
        summary.passed, summary.warned, w_label, summary.failed
    )
}

pub fn to_json_output(results: &[CheckResult], summary: &Summary) -> String {
    #[derive(Serialize)]
    struct Output<'a> {
        checks: &'a [CheckResult],
        summary: &'a Summary,
    }
    serde_json::to_string_pretty(&Output {
        checks: results,
        summary,
    })
    .unwrap_or_else(|_| "{}".to_string())
}

// ─── Check implementations ────────────────────────────────────────────────────

/// Check that `autumn.toml` content parses cleanly (pure, injectable for tests).
pub fn check_toml_content(content: &str) -> CheckResult {
    match toml::from_str::<toml::Table>(content) {
        Err(e) => CheckResult {
            name: "autumn_toml",
            status: CheckStatus::Fail,
            detail: Some(e.to_string()),
            hint: Some("Fix the syntax error in autumn.toml"),
        },
        Ok(table) => {
            let unknown: Vec<String> = table
                .keys()
                .filter(|k| !KNOWN_TOML_SECTIONS.contains(&k.as_str()))
                .cloned()
                .collect();
            if unknown.is_empty() {
                CheckResult {
                    name: "autumn_toml",
                    status: CheckStatus::Pass,
                    detail: Some("autumn.toml is valid".into()),
                    hint: None,
                }
            } else {
                CheckResult {
                    name: "autumn_toml",
                    status: CheckStatus::Warn,
                    detail: Some(format!("unknown keys: {}", unknown.join(", "))),
                    hint: Some("Remove or rename unrecognised keys in autumn.toml"),
                }
            }
        }
    }
}

/// Compare CLI version against the project's `autumn-web` version (pure, injectable for tests).
///
/// For semver < 1.0 (`0.MINOR.PATCH`), a minor-version mismatch is treated as
/// a breaking incompatibility (Fail); a patch-only mismatch is a warning.
pub fn check_version_compat(cli_version: &str, web_version: &str) -> CheckResult {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        // Strip leading `=`, `^`, `~` requirement operators if present.
        let v = v.trim().trim_start_matches(['=', '^', '~', ' ']);
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        let major: u64 = parts[0].parse().ok()?;
        let minor: u64 = parts[1].parse().ok()?;
        let patch: u64 = if parts.len() >= 3 {
            parts[2]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0)
        } else {
            0
        };
        Some((major, minor, patch))
    };

    let Some(cli) = parse(cli_version) else {
        return CheckResult {
            name: "version_compat",
            status: CheckStatus::Warn,
            detail: Some(format!("cannot parse CLI version: {cli_version}")),
            hint: Some("Reinstall autumn-cli"),
        };
    };
    let Some(web) = parse(web_version) else {
        return CheckResult {
            name: "version_compat",
            status: CheckStatus::Warn,
            detail: Some(format!("cannot parse autumn-web version: {web_version}")),
            hint: Some("Check autumn-web version in Cargo.toml"),
        };
    };

    if cli.0 != web.0 || cli.1 != web.1 {
        CheckResult {
            name: "version_compat",
            status: CheckStatus::Fail,
            detail: Some(format!(
                "autumn-cli {cli_version} is incompatible with autumn-web {web_version}"
            )),
            hint: Some(
                "Run `cargo install --path autumn-cli` to match your project's autumn-web version",
            ),
        }
    } else if cli.2 != web.2 {
        CheckResult {
            name: "version_compat",
            status: CheckStatus::Warn,
            detail: Some(format!(
                "autumn-cli {cli_version} vs autumn-web {web_version} (patch skew)"
            )),
            hint: Some("Consider updating either the CLI or your project's autumn-web dependency"),
        }
    } else {
        CheckResult {
            name: "version_compat",
            status: CheckStatus::Pass,
            detail: Some(format!(
                "autumn-cli {cli_version} matches autumn-web {web_version}"
            )),
            hint: None,
        }
    }
}

/// Check whether a port is bindable using an injectable binding function.
pub fn check_port_bindable_impl(port: u16, try_bind: impl Fn(u16) -> bool) -> CheckResult {
    if try_bind(port) {
        CheckResult {
            name: "port_bindable",
            status: CheckStatus::Pass,
            detail: Some(format!("port {port} is available")),
            hint: None,
        }
    } else {
        CheckResult {
            name: "port_bindable",
            status: CheckStatus::Fail,
            detail: Some(format!("port {port} is already in use")),
            hint: Some("Kill the process using that port, or change server.port in autumn.toml"),
        }
    }
}

/// Compare version strings for the Rust toolchain check (pure, injectable for tests).
pub fn check_rust_toolchain_impl(current_output: &str, required: &str) -> CheckResult {
    let parse_ver = |s: &str| -> Option<(u64, u64, u64)> {
        let s = s.trim();
        let s = s
            .strip_prefix("rustc ")
            .map_or(s, |rest| rest.split_whitespace().next().unwrap_or(rest));
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() < 3 {
            return None;
        }
        let major: u64 = parts[0].parse().ok()?;
        let minor: u64 = parts[1].parse().ok()?;
        let patch: u64 = parts[2]
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        Some((major, minor, patch))
    };

    let Some(cur) = parse_ver(current_output) else {
        return CheckResult {
            name: "rust_toolchain",
            status: CheckStatus::Warn,
            detail: Some(format!("cannot parse rustc version: {current_output}")),
            hint: Some("Run `rustup update` to ensure a known Rust version"),
        };
    };
    let Some(req) = parse_ver(required) else {
        return CheckResult {
            name: "rust_toolchain",
            status: CheckStatus::Warn,
            detail: Some(format!("cannot parse MSRV: {required}")),
            hint: None,
        };
    };

    if cur >= req {
        CheckResult {
            name: "rust_toolchain",
            status: CheckStatus::Pass,
            detail: Some(format!(
                "rustc {}.{}.{} ≥ MSRV {}.{}.{}",
                cur.0, cur.1, cur.2, req.0, req.1, req.2
            )),
            hint: None,
        }
    } else {
        CheckResult {
            name: "rust_toolchain",
            status: CheckStatus::Fail,
            detail: Some(format!(
                "rustc {}.{}.{} < MSRV {}.{}.{}",
                cur.0, cur.1, cur.2, req.0, req.1, req.2
            )),
            hint: Some("Run `rustup update stable` to upgrade your Rust toolchain"),
        }
    }
}

// ─── IO-dependent checks ──────────────────────────────────────────────────────

fn check_rust_toolchain(msrv: &str) -> CheckResult {
    match std::process::Command::new("rustc")
        .arg("--version")
        .output()
    {
        Ok(out) if out.status.success() => {
            let ver = String::from_utf8_lossy(&out.stdout).into_owned();
            check_rust_toolchain_impl(ver.trim(), msrv)
        }
        _ => CheckResult {
            name: "rust_toolchain",
            status: CheckStatus::Fail,
            detail: Some("`rustc --version` failed".into()),
            hint: Some("Install Rust via https://rustup.rs/"),
        },
    }
}

fn check_port_bindable(port: u16) -> CheckResult {
    check_port_bindable_impl(port, |p| {
        std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()
    })
}

/// Check whether the Tailwind binary is present and runnable (injectable for tests).
///
/// `exists` and `can_run` are separate so that `can_run` is never invoked when
/// the file is absent — avoiding a spurious "permission denied" OS error.
pub fn check_tailwind_binary_impl(
    path: &std::path::Path,
    exists: impl Fn(&std::path::Path) -> bool,
    can_run: impl Fn(&std::path::Path) -> bool,
) -> CheckResult {
    if !exists(path) {
        return CheckResult {
            name: "tailwind_binary",
            status: CheckStatus::Fail,
            detail: Some(format!("{} not found", path.display())),
            hint: Some("Run `autumn setup` to download the Tailwind CSS binary"),
        };
    }
    if can_run(path) {
        CheckResult {
            name: "tailwind_binary",
            status: CheckStatus::Pass,
            detail: Some(format!("{} is present and runnable", path.display())),
            hint: None,
        }
    } else {
        CheckResult {
            name: "tailwind_binary",
            status: CheckStatus::Fail,
            detail: Some(format!("{} exists but is not runnable", path.display())),
            hint: Some("Run `autumn setup --force` to re-download the Tailwind CSS binary"),
        }
    }
}

fn check_tailwind_binary() -> CheckResult {
    let path = if cfg!(windows) {
        std::path::PathBuf::from("target/autumn/tailwindcss.exe")
    } else {
        std::path::PathBuf::from("target/autumn/tailwindcss")
    };
    check_tailwind_binary_impl(&path, std::path::Path::exists, |p| {
        // Try to invoke the binary; Ok(_) means the OS could execute it
        // regardless of exit code (--help may return 0 or 1 depending on version).
        std::process::Command::new(p).arg("--help").output().is_ok()
    })
}

fn check_stale_artifacts() -> CheckResult {
    let cargo_lock = std::path::Path::new("Cargo.lock");
    let dist = std::path::Path::new("dist");
    let target = std::path::Path::new("target");

    let lock_mtime = cargo_lock.metadata().and_then(|m| m.modified()).ok();

    let dir_older_than_lock = |dir: &std::path::Path| -> bool {
        let Some(lock_t) = lock_mtime else {
            return false;
        };
        dir.metadata()
            .and_then(|m| m.modified())
            .map(|dir_t| dir_t < lock_t)
            .unwrap_or(false)
    };

    let dist_stale = dist.exists() && dir_older_than_lock(dist);
    let target_stale = target.exists() && dir_older_than_lock(target);

    if dist_stale || target_stale {
        let which: Vec<&str> = [
            dist_stale.then_some("dist/"),
            target_stale.then_some("target/"),
        ]
        .into_iter()
        .flatten()
        .collect();
        CheckResult {
            name: "stale_artifacts",
            status: CheckStatus::Warn,
            detail: Some(format!(
                "{} may be stale relative to Cargo.lock",
                which.join(", ")
            )),
            hint: Some("Run `cargo build` or `autumn build` to refresh artifacts"),
        }
    } else {
        CheckResult {
            name: "stale_artifacts",
            status: CheckStatus::Pass,
            detail: Some("artifacts look fresh".into()),
            hint: None,
        }
    }
}

/// Read the `rust-version` MSRV from the nearest workspace/package `Cargo.toml`.
fn read_msrv() -> Option<String> {
    let content = std::fs::read_to_string("Cargo.toml").ok()?;
    let table: toml::Table = toml::from_str(&content).ok()?;

    // Workspace: [workspace.package] rust-version
    if let Some(ver) = table
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("rust-version"))
        .and_then(|v| v.as_str())
    {
        return Some(ver.to_owned());
    }

    // Plain package: [package] rust-version
    table
        .get("package")
        .and_then(|p| p.get("rust-version"))
        .and_then(|v| v.as_str())
        .map(std::borrow::ToOwned::to_owned)
}

/// Read the `autumn-web` version requirement from the project's `Cargo.toml`.
fn read_autumn_web_version() -> Option<String> {
    let content = std::fs::read_to_string("Cargo.toml").ok()?;
    let table: toml::Table = toml::from_str(&content).ok()?;

    let find_in_deps = |deps: &toml::Value| -> Option<String> {
        let entry = deps.get("autumn-web")?;
        match entry {
            toml::Value::String(v) => Some(v.clone()),
            toml::Value::Table(t) => t
                .get("version")?
                .as_str()
                .map(std::borrow::ToOwned::to_owned),
            _ => None,
        }
    };

    // [dependencies] then [workspace.dependencies]
    table
        .get("dependencies")
        .and_then(find_in_deps)
        .or_else(|| {
            table
                .get("workspace")
                .and_then(|w| w.get("dependencies"))
                .and_then(find_in_deps)
        })
}

/// Try to TCP-connect to a host:port within a short timeout.
fn tcp_reachable(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    use std::time::Duration;
    let addrs: Vec<_> = match format!("{host}:{port}").to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => return false,
    };
    addrs
        .iter()
        .any(|addr| std::net::TcpStream::connect_timeout(addr, Duration::from_secs(1)).is_ok())
}

fn check_db_connectivity(database_url: &str) -> CheckResult {
    // Parse host and port from a Postgres URL: postgres://user:pass@host:port/db
    let parsed = parse_db_host_port(database_url);
    match parsed {
        None => CheckResult {
            name: "db_connectivity",
            status: CheckStatus::Warn,
            detail: Some("cannot parse database URL to extract host:port".into()),
            hint: Some("Ensure database.url in autumn.toml is a valid PostgreSQL URL"),
        },
        Some((host, port)) => {
            if tcp_reachable(&host, port) {
                CheckResult {
                    name: "db_connectivity",
                    status: CheckStatus::Pass,
                    detail: Some(format!("Postgres reachable at {host}:{port}")),
                    hint: None,
                }
            } else {
                CheckResult {
                    name: "db_connectivity",
                    status: CheckStatus::Fail,
                    detail: Some(format!("cannot connect to Postgres at {host}:{port}")),
                    hint: Some("Start Postgres and verify database.url in autumn.toml"),
                }
            }
        }
    }
}

fn check_pending_migrations() -> CheckResult {
    match std::process::Command::new("diesel")
        .args(["migration", "pending"])
        .output()
    {
        Err(_) => CheckResult {
            name: "pending_migrations",
            status: CheckStatus::Warn,
            detail: Some("diesel CLI not found; cannot check pending migrations".into()),
            hint: Some(
                "Install diesel_cli: `cargo install diesel_cli --no-default-features --features postgres`",
            ),
        },
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let pending = stdout.lines().filter(|l| !l.trim().is_empty()).count();
            if pending == 0 {
                CheckResult {
                    name: "pending_migrations",
                    status: CheckStatus::Pass,
                    detail: Some("no pending migrations".into()),
                    hint: None,
                }
            } else {
                CheckResult {
                    name: "pending_migrations",
                    status: CheckStatus::Warn,
                    detail: Some(format!("{pending} pending migration(s)")),
                    hint: Some("Run `autumn migrate` to apply pending migrations"),
                }
            }
        }
        Ok(_) => CheckResult {
            name: "pending_migrations",
            status: CheckStatus::Warn,
            detail: Some("diesel migration pending returned non-zero".into()),
            hint: Some("Run `autumn migrate` to apply pending migrations"),
        },
    }
}

/// Parse (host, port) from a Postgres connection URL.
pub fn parse_db_host_port(url: &str) -> Option<(String, u16)> {
    // Expect: postgres://[user:pass@]host[:port]/db
    let without_scheme = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))?;

    // Drop everything after the first `/` (the database name).
    let authority = without_scheme.split('/').next()?;

    // Drop user:pass@ prefix if present.
    let host_port = authority
        .rfind('@')
        .map_or(authority, |at| &authority[at + 1..]);

    // Split host:port.
    if let Some(colon) = host_port.rfind(':') {
        let host = &host_port[..colon];
        let port: u16 = host_port[colon + 1..].parse().ok()?;
        Some((host.to_owned(), port))
    } else {
        Some((host_port.to_owned(), 5432))
    }
}

/// Resolve the configured HTTP port from `autumn.toml` (fallback: 3000).
fn resolve_server_port() -> u16 {
    std::fs::read_to_string("autumn.toml")
        .ok()
        .and_then(|c| toml::from_str::<toml::Table>(&c).ok())
        .and_then(|t| {
            t.get("server")
                .and_then(|s| s.get("port"))
                .and_then(toml::Value::as_integer)
                .and_then(|p| u16::try_from(p).ok())
        })
        .unwrap_or(3000)
}

/// Resolve the optional database URL from env and `autumn.toml`.
fn resolve_optional_db_url() -> Option<String> {
    for var in ["AUTUMN_DATABASE__URL", "DATABASE_URL"] {
        if let Ok(url) = std::env::var(var)
            && !url.is_empty()
        {
            return Some(url);
        }
    }

    std::fs::read_to_string("autumn.toml")
        .ok()
        .and_then(|c| toml::from_str::<toml::Table>(&c).ok())
        .and_then(|t| {
            t.get("database")
                .and_then(|db| db.get("url"))
                .and_then(toml::Value::as_str)
                .filter(|u| !u.is_empty())
                .map(std::borrow::ToOwned::to_owned)
        })
}

/// Check whether Tailwind is enabled in `autumn.toml`.
///
/// Falls back to a heuristic: if `build.rs` exists the project is assumed to
/// use the Tailwind build pipeline.
fn tailwind_enabled() -> bool {
    std::fs::read_to_string("autumn.toml")
        .ok()
        .and_then(|c| toml::from_str::<toml::Table>(&c).ok())
        .and_then(|t| t.get("tailwind").and_then(toml::Value::as_bool))
        .unwrap_or_else(|| std::path::Path::new("build.rs").exists())
}

// ─── Main entry point ─────────────────────────────────────────────────────────

/// Run all doctor checks and report results.
///
/// Checks are organised in two phases:
/// 1. **Config phase** (serial, fast) — file/env reads to decide which checks
///    to run and what data to pass them.
/// 2. **Check phase** (parallel) — every applicable check is spawned on its own
///    thread so that slow operations (TCP connect, subprocess calls) overlap.
///    Results are joined back in display order.
pub fn run(opts: DoctorOptions) {
    use std::thread;
    type Task = Box<dyn FnOnce() -> CheckResult + Send>;

    let cli_version = env!("CARGO_PKG_VERSION");

    if !opts.json {
        println!("\u{1F342} autumn doctor\n");
    }

    // ── Phase 1: config reads (serial, cheap) ────────────────────────────────
    let msrv = read_msrv().unwrap_or_else(|| "1.88.0".to_owned());
    let web_ver = read_autumn_web_version();
    let toml_result = std::fs::read_to_string("autumn.toml");
    let db_url = resolve_optional_db_url();
    let port = resolve_server_port();
    let tailwind = tailwind_enabled();

    // ── Phase 2: build tasks in display order ────────────────────────────────
    let mut tasks: Vec<Task> = Vec::new();

    // 1. Rust toolchain
    tasks.push(Box::new(move || check_rust_toolchain(&msrv)));

    // 2. Version skew (only when autumn-web appears in the project's Cargo.toml)
    if let Some(web) = web_ver {
        tasks.push(Box::new(move || check_version_compat(cli_version, &web)));
    }

    // 3. autumn.toml
    match toml_result {
        Ok(content) => tasks.push(Box::new(move || check_toml_content(&content))),
        Err(_) => tasks.push(Box::new(|| CheckResult {
            name: "autumn_toml",
            status: CheckStatus::Warn,
            detail: Some("autumn.toml not found in current directory".into()),
            hint: Some("Run `autumn doctor` from your project root (where autumn.toml lives)"),
        })),
    }

    // 4 & 5. Database (only when configured)
    if let Some(url) = db_url {
        tasks.push(Box::new(move || check_db_connectivity(&url)));
        tasks.push(Box::new(check_pending_migrations));
    }

    // 6. Port bindable
    tasks.push(Box::new(move || check_port_bindable(port)));

    // 7. Tailwind binary (only when build pipeline is present)
    if tailwind {
        tasks.push(Box::new(check_tailwind_binary));
    }

    // 8. Stale artifacts (warn only, never fail)
    tasks.push(Box::new(check_stale_artifacts));

    // ── Phase 3: spawn all tasks concurrently ────────────────────────────────
    let handles: Vec<thread::JoinHandle<CheckResult>> =
        tasks.into_iter().map(thread::spawn).collect();

    // ── Phase 4: join in order (preserves display ordering) ──────────────────
    let results: Vec<CheckResult> = handles
        .into_iter()
        .map(|h| {
            h.join().unwrap_or_else(|_| CheckResult {
                name: "internal_error",
                status: CheckStatus::Fail,
                detail: Some("a check panicked unexpectedly".into()),
                hint: None,
            })
        })
        .collect();

    let summary = compute_summary(&results);
    let code = exit_code(&summary, opts.strict);

    if opts.json {
        println!("{}", to_json_output(&results, &summary));
    } else {
        for r in &results {
            println!("{}", format_check_line(r));
        }
        println!();
        println!("{}", format_summary_line(&summary, code));
    }

    std::process::exit(code);
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── glyph ────────────────────────────────────────────────────────────────

    #[test]
    fn glyph_pass() {
        assert_eq!(glyph(&CheckStatus::Pass), "✅");
    }

    #[test]
    fn glyph_warn() {
        assert_eq!(glyph(&CheckStatus::Warn), "⚠️ ");
    }

    #[test]
    fn glyph_fail() {
        assert_eq!(glyph(&CheckStatus::Fail), "❌");
    }

    // ── compute_summary ──────────────────────────────────────────────────────

    #[test]
    fn compute_summary_all_pass() {
        let results = vec![
            CheckResult {
                name: "a",
                status: CheckStatus::Pass,
                detail: None,
                hint: None,
            },
            CheckResult {
                name: "b",
                status: CheckStatus::Pass,
                detail: None,
                hint: None,
            },
        ];
        let s = compute_summary(&results);
        assert_eq!(s.passed, 2);
        assert_eq!(s.warned, 0);
        assert_eq!(s.failed, 0);
    }

    #[test]
    fn compute_summary_mixed() {
        let results = vec![
            CheckResult {
                name: "a",
                status: CheckStatus::Pass,
                detail: None,
                hint: None,
            },
            CheckResult {
                name: "b",
                status: CheckStatus::Warn,
                detail: None,
                hint: None,
            },
            CheckResult {
                name: "c",
                status: CheckStatus::Fail,
                detail: None,
                hint: None,
            },
        ];
        let s = compute_summary(&results);
        assert_eq!(s.passed, 1);
        assert_eq!(s.warned, 1);
        assert_eq!(s.failed, 1);
    }

    #[test]
    fn compute_summary_empty() {
        let s = compute_summary(&[]);
        assert_eq!(s.passed, 0);
        assert_eq!(s.warned, 0);
        assert_eq!(s.failed, 0);
    }

    // ── exit_code ────────────────────────────────────────────────────────────

    #[test]
    fn exit_code_no_failures() {
        let s = Summary {
            passed: 3,
            warned: 0,
            failed: 0,
        };
        assert_eq!(exit_code(&s, false), 0);
    }

    #[test]
    fn exit_code_with_failure() {
        let s = Summary {
            passed: 2,
            warned: 0,
            failed: 1,
        };
        assert_eq!(exit_code(&s, false), 1);
    }

    #[test]
    fn exit_code_warn_non_strict() {
        let s = Summary {
            passed: 2,
            warned: 1,
            failed: 0,
        };
        assert_eq!(exit_code(&s, false), 0);
    }

    #[test]
    fn exit_code_warn_strict() {
        let s = Summary {
            passed: 2,
            warned: 1,
            failed: 0,
        };
        assert_eq!(exit_code(&s, true), 1);
    }

    #[test]
    fn exit_code_zero_when_all_pass_strict() {
        let s = Summary {
            passed: 5,
            warned: 0,
            failed: 0,
        };
        assert_eq!(exit_code(&s, true), 0);
    }

    // ── format_check_line ────────────────────────────────────────────────────

    #[test]
    fn format_check_line_pass_contains_glyph_and_name() {
        let r = CheckResult {
            name: "rust_toolchain",
            status: CheckStatus::Pass,
            detail: Some("1.88.0".into()),
            hint: None,
        };
        let line = format_check_line(&r);
        assert!(line.contains("✅"));
        assert!(line.contains("rust_toolchain"));
    }

    #[test]
    fn format_check_line_fail_includes_hint() {
        let r = CheckResult {
            name: "port_bindable",
            status: CheckStatus::Fail,
            detail: Some("port 3000 in use".into()),
            hint: Some("Kill the process using port 3000 or change server.port in autumn.toml"),
        };
        let line = format_check_line(&r);
        assert!(line.contains("❌"));
        assert!(line.contains("port_bindable"));
        assert!(line.contains("Kill the process"));
    }

    #[test]
    fn format_check_line_pass_omits_hint() {
        let r = CheckResult {
            name: "rust_toolchain",
            status: CheckStatus::Pass,
            detail: None,
            hint: Some("some hint that should not appear on pass"),
        };
        let line = format_check_line(&r);
        assert!(!line.contains("some hint"));
    }

    #[test]
    fn format_check_line_warn_includes_hint() {
        let r = CheckResult {
            name: "version_compat",
            status: CheckStatus::Warn,
            detail: Some("patch skew".into()),
            hint: Some("Update your dependency"),
        };
        let line = format_check_line(&r);
        assert!(line.contains("⚠️"));
        assert!(line.contains("Update your dependency"));
    }

    // ── format_summary_line ──────────────────────────────────────────────────

    #[test]
    fn format_summary_all_pass() {
        let s = Summary {
            passed: 7,
            warned: 0,
            failed: 0,
        };
        let line = format_summary_line(&s, 0);
        assert!(line.contains("7 passed"));
        assert!(line.contains("0 warnings"));
        assert!(line.contains("0 failed"));
        assert!(line.contains("all clear"));
    }

    #[test]
    fn format_summary_with_failure() {
        let s = Summary {
            passed: 5,
            warned: 1,
            failed: 1,
        };
        let line = format_summary_line(&s, 1);
        assert!(line.contains("5 passed"));
        assert!(line.contains("1 warning"));
        assert!(line.contains("1 failed"));
        assert!(line.contains("problems found"));
    }

    #[test]
    fn format_summary_singular_warning_label() {
        let s = Summary {
            passed: 3,
            warned: 1,
            failed: 0,
        };
        let line = format_summary_line(&s, 0);
        assert!(line.contains("1 warning,"));
    }

    #[test]
    fn format_summary_plural_warning_label() {
        let s = Summary {
            passed: 1,
            warned: 2,
            failed: 0,
        };
        let line = format_summary_line(&s, 0);
        assert!(line.contains("2 warnings,"));
    }

    // ── check_toml_content ───────────────────────────────────────────────────

    #[test]
    fn check_toml_content_valid() {
        let content = r#"
[server]
port = 3000

[database]
url = "postgres://localhost/mydb"
"#;
        let r = check_toml_content(content);
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_toml_content_syntax_error() {
        let content = "[[[[invalid toml";
        let r = check_toml_content(content);
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.hint.is_some());
    }

    #[test]
    fn check_toml_content_unknown_key_warns() {
        let content = r#"
[server]
port = 3000

[totally_unknown_section]
foo = "bar"
"#;
        let r = check_toml_content(content);
        assert_eq!(r.status, CheckStatus::Warn);
        assert!(
            r.detail
                .as_deref()
                .unwrap_or("")
                .contains("totally_unknown_section"),
            "detail should mention the unknown key"
        );
    }

    #[test]
    fn check_toml_content_empty_is_pass() {
        let r = check_toml_content("");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_toml_content_all_known_sections_pass() {
        let mut content = String::new();
        for section in KNOWN_TOML_SECTIONS {
            content.push_str(&format!("[{section}]\n"));
        }
        let r = check_toml_content(&content);
        assert_eq!(r.status, CheckStatus::Pass);
    }

    // ── check_version_compat ─────────────────────────────────────────────────

    #[test]
    fn check_version_compat_matching() {
        let r = check_version_compat("0.3.0", "0.3.0");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_version_compat_minor_skew_fails() {
        let r = check_version_compat("0.3.0", "0.4.0");
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.hint.is_some());
    }

    #[test]
    fn check_version_compat_patch_skew_warns() {
        let r = check_version_compat("0.3.0", "0.3.1");
        assert_eq!(r.status, CheckStatus::Warn);
    }

    #[test]
    fn check_version_compat_caret_requirement_stripped() {
        let r = check_version_compat("0.3.0", "^0.3");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_version_compat_tilde_requirement_stripped() {
        let r = check_version_compat("0.3.0", "~0.3.0");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_version_compat_exact_requirement_stripped() {
        let r = check_version_compat("0.3.0", "=0.3.0");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    // ── check_port_bindable_impl ─────────────────────────────────────────────

    #[test]
    fn check_port_bindable_impl_success() {
        let r = check_port_bindable_impl(3000, |_port| true);
        assert_eq!(r.status, CheckStatus::Pass);
        assert!(r.detail.as_deref().unwrap_or("").contains("available"));
    }

    #[test]
    fn check_port_bindable_impl_failure() {
        let r = check_port_bindable_impl(3000, |_port| false);
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.hint.is_some());
        assert!(r.detail.as_deref().unwrap_or("").contains("in use"));
    }

    #[test]
    fn check_port_bindable_impl_reports_correct_port() {
        let r = check_port_bindable_impl(8080, |_| true);
        assert!(r.detail.as_deref().unwrap_or("").contains("8080"));
    }

    // ── check_rust_toolchain_impl ────────────────────────────────────────────

    #[test]
    fn check_rust_toolchain_impl_pass() {
        let r = check_rust_toolchain_impl("rustc 1.88.0 (abc123 2026-04-01)", "1.88.0");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_rust_toolchain_impl_fail() {
        let r = check_rust_toolchain_impl("rustc 1.80.0 (abc123 2025-01-01)", "1.88.0");
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.hint.is_some());
    }

    #[test]
    fn check_rust_toolchain_impl_newer_passes() {
        let r = check_rust_toolchain_impl("rustc 1.90.0 (abc 2026-01-01)", "1.88.0");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_rust_toolchain_impl_exact_msrv_passes() {
        let r = check_rust_toolchain_impl("1.88.0", "1.88.0");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn check_rust_toolchain_impl_unparseable_warns() {
        let r = check_rust_toolchain_impl("not-a-version", "1.88.0");
        assert_eq!(r.status, CheckStatus::Warn);
    }

    // ── parse_db_host_port ───────────────────────────────────────────────────

    #[test]
    fn parse_db_host_port_full_url() {
        let (host, port) = parse_db_host_port("postgres://user:pass@localhost:5432/mydb").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 5432);
    }

    #[test]
    fn parse_db_host_port_no_credentials() {
        let (host, port) = parse_db_host_port("postgres://localhost/mydb").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 5432);
    }

    #[test]
    fn parse_db_host_port_default_port() {
        let (host, port) = parse_db_host_port("postgres://user:pass@db.example.com/mydb").unwrap();
        assert_eq!(host, "db.example.com");
        assert_eq!(port, 5432);
    }

    #[test]
    fn parse_db_host_port_custom_port() {
        let (host, port) = parse_db_host_port("postgres://localhost:6543/test").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 6543);
    }

    #[test]
    fn parse_db_host_port_postgresql_scheme() {
        let (host, port) = parse_db_host_port("postgresql://localhost:5432/db").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 5432);
    }

    #[test]
    fn parse_db_host_port_invalid_scheme() {
        assert!(parse_db_host_port("mysql://localhost/db").is_none());
    }

    // ── check_tailwind_binary_impl ───────────────────────────────────────────

    #[test]
    fn check_tailwind_not_found() {
        let r = check_tailwind_binary_impl(
            std::path::Path::new("target/autumn/tailwindcss"),
            |_| false,
            |_| false,
        );
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.detail.as_deref().unwrap_or("").contains("not found"));
        assert!(r.hint.unwrap_or("").contains("autumn setup"));
    }

    #[test]
    fn check_tailwind_exists_but_not_runnable() {
        let r = check_tailwind_binary_impl(
            std::path::Path::new("target/autumn/tailwindcss"),
            |_| true,
            |_| false,
        );
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.detail.as_deref().unwrap_or("").contains("not runnable"));
        assert!(r.hint.unwrap_or("").contains("--force"));
    }

    #[test]
    fn check_tailwind_exists_and_runnable() {
        let r = check_tailwind_binary_impl(
            std::path::Path::new("target/autumn/tailwindcss"),
            |_| true,
            |_| true,
        );
        assert_eq!(r.status, CheckStatus::Pass);
        assert!(r.detail.as_deref().unwrap_or("").contains("runnable"));
    }

    #[test]
    fn check_tailwind_not_found_does_not_invoke_run_fn() {
        let run_called = std::cell::Cell::new(false);
        let _ = check_tailwind_binary_impl(
            std::path::Path::new("target/autumn/tailwindcss"),
            |_| false,
            |_| {
                run_called.set(true);
                false
            },
        );
        assert!(
            !run_called.get(),
            "run_fn must not be called when path doesn't exist"
        );
    }

    // ── to_json_output ───────────────────────────────────────────────────────

    #[test]
    fn json_output_contains_checks_and_summary() {
        let results = vec![
            CheckResult {
                name: "rust_toolchain",
                status: CheckStatus::Pass,
                detail: Some("1.88.0".into()),
                hint: None,
            },
            CheckResult {
                name: "port_bindable",
                status: CheckStatus::Fail,
                detail: None,
                hint: Some("hint text"),
            },
        ];
        let summary = compute_summary(&results);
        let json = to_json_output(&results, &summary);

        assert!(json.contains("rust_toolchain"));
        assert!(json.contains("port_bindable"));
        assert!(json.contains("\"passed\": 1"));
        assert!(json.contains("\"failed\": 1"));
    }

    #[test]
    fn json_output_valid_json() {
        let results = vec![CheckResult {
            name: "test",
            status: CheckStatus::Pass,
            detail: None,
            hint: None,
        }];
        let summary = compute_summary(&results);
        let json = to_json_output(&results, &summary);
        // Should parse as valid JSON
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
    }
}
