//! `autumn migrate` -- run, inspect, or roll back Diesel database migrations.
//!
//! # Subcommands
//!
//! | Subcommand | Description |
//! |---|---|
//! | *(default)* | Apply all pending user + framework migrations |
//! | `status` | Show applied/pending status per migration plus rollback availability |
//! | `check` | Classify every `up.sql` and `down.sql` for rolling-deploy safety |
//! | `down` | Revert the most recently applied user migration(s) via `down.sql` |
//!
//! # User vs framework migrations
//!
//! **User migrations** live in `./migrations/` and are executed via the
//! `diesel` CLI (`diesel migration run`) or the Rust harness (for `down`).
//!
//! **Framework migrations** are embedded in the `autumn` crate and applied
//! through the Rust `MigrationHarness`. They are **forward-only**: `autumn
//! migrate down` never reverts them. Their forward-only contract is preserved
//! regardless of which user migrations are rolled back.
//!
//! # Database URL resolution
//!
//! Precedence (highest to lowest):
//! 1. `AUTUMN_DATABASE__PRIMARY_URL` env var
//! 2. `AUTUMN_DATABASE__URL` env var
//! 3. `DATABASE_URL` env var
//! 4. `database.primary_url` in `autumn.toml`
//! 5. `database.url` in `autumn.toml`

pub mod safety;

use std::path::Path;
use std::process::Command;

use autumn_web::migrate::{
    AppliedUserMigration, EmbeddedMigrations, FRAMEWORK_MIGRATIONS, MigrationError, MigrationResult,
};

/// Default directory containing Diesel migration files.
const DEFAULT_MIGRATIONS_DIR: &str = "migrations";

/// Arguments for `autumn migrate down`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownArgs {
    /// Revert this many migrations (default: 1). Mutually exclusive with `to`.
    pub steps: Option<usize>,
    /// Revert until this version is the latest applied. Mutually exclusive with `steps`.
    pub to: Option<String>,
    /// Required when targeting the production profile.
    pub yes_i_mean_prod: bool,
}

/// Subcommands for `autumn migrate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateAction {
    /// Run all pending migrations.
    Run,
    /// Show migration status (pending / applied).
    Status,
    /// Preflight safety check — classifies all migration SQL files and returns
    /// a non-zero exit code if any unsafe or unclassified operations are found.
    Check,
    /// Revert the most recently applied user migration(s).
    Down(DownArgs),
}

/// Per-migration safety report returned by [`check_migrations_in_dir`].
pub struct MigrationSafetyReport {
    /// Migration directory name (e.g. `"20260101000000_create_posts"`).
    pub name: String,
    /// Findings for `up.sql`.
    pub up: Vec<safety::SafetyFinding>,
    /// Findings for `down.sql` (empty when `down.sql` is absent or empty).
    pub down: Vec<safety::SafetyFinding>,
}

/// Which databases `autumn migrate` operates on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateTarget {
    /// The control database plus every `[[database.shards]]` entry
    /// (control first, shards in declaration order). The default.
    All,
    /// Only the control database (`database.primary_url`/`url`) —
    /// pre-shards behavior.
    ControlOnly,
    /// A single shard, addressed by its configured `name`.
    Shard(String),
}

/// Run the migrate command.
pub fn run(action: &MigrateAction, with_maintenance: bool, target: &MigrateTarget) {
    eprintln!("\u{1F342} autumn migrate\n");

    match action {
        MigrateAction::Check => {
            let migrations_dir = resolve_migrations_dir();
            run_safety_check(&migrations_dir);
            return;
        }
        MigrateAction::Down(args) => {
            run_down(args, with_maintenance, target);
            return;
        }
        _ => {}
    }

    // 1. Resolve migration target databases from autumn.toml + env
    let targets = resolve_targets(target);

    // 2. Resolve migrations directory
    let migrations_dir = resolve_migrations_dir();

    // 3. Check that diesel CLI is available
    check_diesel_cli();

    // 4. Enable maintenance mode if requested
    if with_maintenance && *action == MigrateAction::Run {
        enable_maintenance_for_migrate();
    }

    // 5. Execute the appropriate diesel command per target
    match action {
        MigrateAction::Run => {
            run_all_targets(&targets, &migrations_dir, with_maintenance);
        }
        MigrateAction::Status => {
            for (label, url) in &targets {
                eprintln!("\u{2500}\u{2500} {label} \u{2500}\u{2500}");
                show_status(url, &migrations_dir);
                show_rollback_availability(url, &migrations_dir);
                show_framework_status(url);
                eprintln!();
            }
        }
        MigrateAction::Check | MigrateAction::Down(_) => unreachable!("handled above"),
    }
}

/// Resolve the `(label, database_url)` pairs the command operates on,
/// in apply order (control first, then shards in declaration order).
fn resolve_targets(target: &MigrateTarget) -> Vec<(String, String)> {
    let control = try_resolve_database_url();
    let shards = resolve_shard_database_urls();
    match build_targets(control, shards, target) {
        Ok(targets) => targets,
        Err(message) => {
            eprintln!("{message}");
            if matches!(target, MigrateTarget::All | MigrateTarget::ControlOnly) {
                // Reuse the standard missing-URL guidance (prints and exits).
                resolve_database_url();
            }
            std::process::exit(1);
        }
    }
}

/// Pure target-selection logic behind [`resolve_targets`], separated so
/// every branch is unit-testable without touching the environment.
fn build_targets(
    control: Option<String>,
    shards: Vec<(String, String)>,
    target: &MigrateTarget,
) -> Result<Vec<(String, String)>, String> {
    match target {
        MigrateTarget::ControlOnly => control
            .map(|url| vec![("control".to_owned(), url)])
            .ok_or_else(|| "\u{2717} No control database URL found.".to_owned()),
        MigrateTarget::Shard(name) => {
            let Some((_, url)) = shards.iter().find(|(shard, _)| shard == name) else {
                let detail = if shards.is_empty() {
                    "No [[database.shards]] entries found in autumn.toml or environment.".to_owned()
                } else {
                    let known: Vec<&str> = shards.iter().map(|(n, _)| n.as_str()).collect();
                    format!("Known shards: {}", known.join(", "))
                };
                return Err(format!("\u{2717} Unknown shard {name:?}.\n  {detail}"));
            };
            Ok(vec![(format!("shard:{name}"), url.clone())])
        }
        MigrateTarget::All => {
            // Shard-only deployments (no control role) are a valid shape:
            // include the control target only when a control URL resolves.
            let mut targets = Vec::new();
            if let Some(control_url) = control {
                targets.push(("control".to_owned(), control_url));
            } else if shards.is_empty() {
                return Err("\u{2717} No database URL found.".to_owned());
            }
            for (name, url) in shards {
                targets.push((format!("shard:{name}"), url));
            }
            Ok(targets)
        }
    }
}

/// Apply migrations to every target in order, failing fast with a
/// per-target summary.
fn run_all_targets(targets: &[(String, String)], migrations_dir: &str, with_maintenance: bool) {
    let mut completed: Vec<&str> = Vec::new();
    for (label, url) in targets {
        eprintln!("\u{2500}\u{2500} Migrating {label} \u{2500}\u{2500}");
        if run_single_target(url, migrations_dir) {
            completed.push(label);
            eprintln!();
        } else {
            eprintln!();
            eprintln!("  Summary:");
            for done in &completed {
                eprintln!("    \u{2713} {done}");
            }
            eprintln!("    \u{2717} {label} \u{2014} FAILED (see output above)");
            for (pending, _) in targets.iter().skip(completed.len() + 1) {
                eprintln!("    \u{2022} {pending} \u{2014} not attempted");
            }
            eprintln!();
            eprintln!(
                "  Migrations already applied to earlier targets are skipped \
                 idempotently when you rerun `autumn migrate`."
            );
            if with_maintenance {
                eprintln!(
                    "  \u{26A0}\u{FE0F}  Migration failed — maintenance mode left ON for safety."
                );
                eprintln!("      Fix the migration then run `autumn migrate` to retry.");
                eprintln!("      Run `autumn maintenance off` to re-open traffic manually.");
            }
            std::process::exit(1);
        }
    }

    eprintln!("  Summary:");
    for done in completed {
        eprintln!("    \u{2713} {done}");
    }
    if with_maintenance {
        disable_maintenance_after_migrate();
    }
}

/// Enable maintenance mode before a migrate run.
fn enable_maintenance_for_migrate() {
    use autumn_web::maintenance::{MAINTENANCE_FLAG_FILE, MaintenanceConfig, MaintenanceState};
    let path = std::path::Path::new(MAINTENANCE_FLAG_FILE);
    let config = MaintenanceConfig {
        message: Some("Database migration in progress. Please try again in a moment.".to_owned()),
        ..Default::default()
    };
    match MaintenanceState::save_to_file(path, &config) {
        Ok(()) => eprintln!("  \u{26A0}\u{FE0F}  Maintenance mode ENABLED (--with-maintenance)"),
        Err(e) => {
            eprintln!("\u{274C} Failed to enable maintenance mode: {e}");
            std::process::exit(1);
        }
    }
}

/// Disable maintenance mode after a successful migrate run.
fn disable_maintenance_after_migrate() {
    use autumn_web::maintenance::{MAINTENANCE_FLAG_FILE, MaintenanceState};
    let path = std::path::Path::new(MAINTENANCE_FLAG_FILE);
    match MaintenanceState::remove_flag_file(path) {
        Ok(_) => eprintln!("  \u{2713} Maintenance mode DISABLED — normal traffic resuming"),
        Err(e) => eprintln!("\u{26A0}\u{FE0F}  Could not remove maintenance flag: {e}"),
    }
}

/// Apply app + framework migrations to one database. Returns whether
/// everything succeeded; the caller decides how to fail.
fn run_single_target(database_url: &str, migrations_dir: &str) -> bool {
    use autumn_web::migrate::{DEFAULT_LOCK_WAIT_TIMEOUT, hold_migration_lock};

    // Acquire this target database's Postgres advisory lock before reading
    // the pending-migration list. This serializes concurrent callers
    // (rolling-deploy replicas or parallel `autumn migrate run`
    // invocations): only one process runs migrations against a given
    // database at a time; the rest wait and then find no pending work.
    // The lock is released when `_lock_guard` drops (end of this function
    // or process exit — both are safe because PostgreSQL releases
    // session-level advisory locks on connection close).
    //
    // Known limitation: the advisory lock lives on the parent process's
    // connection, not inside the child `diesel` subprocess. If the parent is
    // killed (SIGKILL or SIGTERM) while the child is still running, Postgres
    // releases the session lock and a second caller can acquire it before the
    // child finishes. SIGKILL is not fixable at the Rust level (no destructors
    // run); for SIGTERM a kill-on-drop child guard would close the window but
    // could abort an in-progress transaction. In practice most orchestrators
    // kill the whole cgroup, and Postgres's transaction isolation prevents
    // concurrent dirty writes either way.
    let _lock_guard = match hold_migration_lock(database_url, DEFAULT_LOCK_WAIT_TIMEOUT) {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("\u{274C} Failed to acquire migration lock: {e}");
            return false;
        }
    };

    eprintln!("  Running pending migrations...\n");
    let dir = std::path::Path::new(migrations_dir);
    let status = Command::new("diesel")
        .args(["migration", "run", "--migration-dir"])
        .arg(dir)
        .env("DATABASE_URL", database_url)
        .status();

    match status {
        Ok(s) if s.success() => {
            eprintln!("\n\u{2713} Migrations applied successfully.");
        }
        Ok(_) => {
            eprintln!(
                "\n\u{274C} Migration failed in {}. Check the error output above.",
                dir.display()
            );
            return false;
        }
        Err(e) => {
            eprintln!("\u{274C} Failed to run diesel migration run: {e}");
            return false;
        }
    }

    run_framework_migrations(database_url)
}

/// Print the safety findings for one migration direction (`up.sql`/`down.sql`).
fn print_findings(label: &str, name: &str, findings: &[safety::SafetyFinding]) {
    if safety::is_safe(findings) {
        eprintln!("  \u{2713} {name}  [{label}]");
    } else {
        eprintln!("  \u{2717} {name}  [{label}]");
        for f in findings {
            eprintln!("      \u{2022} {} [{}]", f.operation, f.risk);
            eprintln!("        Why:  {}", f.why);
            eprintln!("        Next: {}", f.next_action);
        }
    }
}

/// Run the migration safety preflight check against all SQL files in `migrations_dir`.
///
/// Prints a human-readable report to stderr and exits with code 1 if any
/// unsafe or potentially-blocking operations are detected in either direction.
fn run_safety_check(migrations_dir: &str) {
    let reports = match check_migrations_in_dir(Path::new(migrations_dir)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("\u{2717} Migration safety check failed: {e}");
            std::process::exit(1);
        }
    };

    if reports.is_empty() {
        eprintln!("\u{2713} No migrations found in {migrations_dir}/");
        return;
    }

    let total = reports.len();
    eprintln!("  Scanning {total} migration(s) in {migrations_dir}/...\n");

    for report in &reports {
        print_findings("up.sql", &report.name, &report.up);
        if !report.down.is_empty() {
            print_findings("down.sql", &report.name, &report.down);
        }
    }

    let any_unsafe = rolling_deploy_blocked(&reports);
    let any_down_unsafe = reports.iter().any(|r| safety::has_unsafe_findings(&r.down));

    eprintln!();
    if any_unsafe {
        eprintln!(
            "\u{2717} One or more migrations contain operations that are unsafe for a live \
             rolling deploy."
        );
        eprintln!("  Review the findings above, apply the expand/contract pattern where needed,");
        eprintln!("  or coordinate a maintenance window before deploying these migrations.");
        std::process::exit(1);
    }
    eprintln!("\u{2713} All {total} migration(s) are safe for a rolling deploy.");
    if any_down_unsafe {
        eprintln!(
            "  Note: some down.sql reverts contain destructive or blocking operations \
             (reported above). These only run on `autumn migrate down`, not on deploy."
        );
    }
}

/// Whether any migration is unsafe for a forward rolling deploy.
///
/// Keys off `up.sql` only — `down.sql` runs on `autumn migrate down`, not during
/// a forward deploy, so its (often inherently destructive) findings do not block
/// the deploy gate.
fn rolling_deploy_blocked(reports: &[MigrationSafetyReport]) -> bool {
    reports.iter().any(|r| safety::has_unsafe_findings(&r.up))
}

/// Read every migration directory in `dir`, classify both `up.sql` and
/// `down.sql`, and return a sorted list of [`MigrationSafetyReport`]s.
///
/// Migration directories that have no `up.sql` are silently skipped.
/// `down.sql` is optional — its findings are empty when the file is absent.
pub fn check_migrations_in_dir(dir: &Path) -> Result<Vec<MigrationSafetyReport>, String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect();

    // Sort by directory name (which starts with a timestamp) for stable output.
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut results = Vec::new();
    for entry in entries {
        let migration_name = entry.file_name().to_string_lossy().into_owned();
        let up_sql_path = entry.path().join("up.sql");
        if !up_sql_path.exists() {
            continue;
        }
        let up_sql = std::fs::read_to_string(&up_sql_path)
            .map_err(|e| format!("cannot read {}: {e}", up_sql_path.display()))?;
        let mut up_findings = safety::classify_sql(&up_sql);
        check_concurrent_index_transaction_opt_out(&up_sql, &entry.path(), &mut up_findings);

        let down_sql_path = entry.path().join("down.sql");
        let down_findings = if down_sql_path.exists() {
            let down_sql = std::fs::read_to_string(&down_sql_path)
                .map_err(|e| format!("cannot read {}: {e}", down_sql_path.display()))?;
            let mut down_findings = safety::classify_sql(&down_sql);
            check_concurrent_index_transaction_opt_out(
                &down_sql,
                &entry.path(),
                &mut down_findings,
            );
            down_findings
        } else {
            Vec::new()
        };

        results.push(MigrationSafetyReport {
            name: migration_name,
            up: up_findings,
            down: down_findings,
        });
    }

    Ok(results)
}

/// If the SQL uses `CREATE INDEX CONCURRENTLY` but the migration directory does not
/// opt out of Diesel's default transaction wrapping via `metadata.toml`, add a
/// `PotentiallyBlocking` finding.
///
/// `PostgreSQL` rejects `CREATE INDEX CONCURRENTLY` inside a transaction block.
/// Without `run_in_transaction = false` in `metadata.toml`, Diesel wraps the
/// migration in a transaction and the deployment job will fail.
fn check_concurrent_index_transaction_opt_out(
    sql: &str,
    migration_dir: &Path,
    findings: &mut Vec<safety::SafetyFinding>,
) {
    if !safety::contains_concurrent_index(sql) {
        return;
    }

    if !migration_opts_out_of_transaction(migration_dir) {
        findings.push(safety::SafetyFinding {
            operation: "CONCURRENTLY index operation (missing transaction opt-out)".to_owned(),
            risk: safety::RiskLevel::PotentiallyBlocking,
            why: "`PostgreSQL` rejects `CREATE INDEX CONCURRENTLY` and `DROP INDEX CONCURRENTLY` \
                  inside a transaction block. Diesel wraps migrations in a transaction by default, \
                  so this migration will fail at deploy time unless the transaction is disabled.",
            next_action: "Add `run_in_transaction = false` to the migration's `metadata.toml` \
                          (create the file if absent). Example: \
                          echo 'run_in_transaction = false' > migrations/<name>/metadata.toml",
        });
    }
}

/// Whether the migration directory's `metadata.toml` sets
/// `run_in_transaction = false`, opting out of Diesel's default per-migration
/// transaction wrapping (required for `CONCURRENTLY` index operations).
fn migration_opts_out_of_transaction(migration_dir: &Path) -> bool {
    std::fs::read_to_string(migration_dir.join("metadata.toml"))
        .ok()
        .and_then(|content| toml::from_str::<toml::Table>(&content).ok())
        .and_then(|table| {
            table
                .get("run_in_transaction")
                .and_then(toml::Value::as_bool)
        })
        .is_some_and(|v| !v)
}

/// Resolve the primary/write database URL from autumn.toml and environment variables.
///
/// Precedence (highest to lowest):
/// 1. `AUTUMN_DATABASE__PRIMARY_URL` environment variable
/// 2. `AUTUMN_DATABASE__URL` environment variable
/// 3. `DATABASE_URL` environment variable
/// 4. `database.primary_url` from `autumn.toml`
/// 5. `database.url` from `autumn.toml`
fn resolve_database_url() -> String {
    resolve_database_url_with_env(|key| std::env::var(key))
}

/// Like [`resolve_database_url`], but returns `None` instead of exiting
/// when no control URL is configured (valid for shard-only deployments).
fn try_resolve_database_url() -> Option<String> {
    let config_table = read_autumn_toml_table();
    resolve_primary_database_url_from_sources(|key| std::env::var(key), config_table.as_ref())
}

fn resolve_database_url_with_env<F>(env_var: F) -> String
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    let config_table = read_autumn_toml_table();
    if let Some(url) = resolve_primary_database_url_from_sources(env_var, config_table.as_ref()) {
        return url;
    }

    eprintln!("\u{2717} No database URL found.");
    eprintln!(
        "  Set database.primary_url (or database.url) in autumn.toml, or set AUTUMN_DATABASE__PRIMARY_URL / AUTUMN_DATABASE__URL / DATABASE_URL."
    );
    std::process::exit(1);
}

fn read_autumn_toml_table() -> Option<toml::Table> {
    let config_path = Path::new("autumn.toml");
    if !config_path.exists() {
        return None;
    }
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|contents| toml::from_str::<toml::Table>(&contents).ok())
}

fn resolve_primary_database_url_from_sources<F>(
    env_var: F,
    table: Option<&toml::Table>,
) -> Option<String>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    for var in [
        "AUTUMN_DATABASE__PRIMARY_URL",
        "AUTUMN_DATABASE__URL",
        "DATABASE_URL",
    ] {
        if let Ok(url) = env_var(var)
            && !url.is_empty()
        {
            return Some(url);
        }
    }

    let database = table?.get("database").and_then(toml::Value::as_table)?;
    for key in ["primary_url", "url"] {
        if let Some(url) = database
            .get(key)
            .and_then(toml::Value::as_str)
            .filter(|url| !url.is_empty())
        {
            return Some(url.to_owned());
        }
    }
    None
}

/// Resolve `(name, primary_url)` for every `[[database.shards]]` entry,
/// in declaration order.
///
/// Mirrors the framework's positional environment override scheme:
/// `AUTUMN_DATABASE__SHARDS__{i}__NAME` / `__PRIMARY_URL` override entry
/// `i` of the TOML declaration (or append a new entry when both are set
/// for the next free index); probing stops at the first absent index.
fn resolve_shard_database_urls() -> Vec<(String, String)> {
    resolve_shard_database_urls_with_env(|key| std::env::var(key))
}

fn resolve_shard_database_urls_with_env<F>(env_var: F) -> Vec<(String, String)>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    let config_table = read_autumn_toml_table();
    resolve_shard_database_urls_from_sources(env_var, config_table.as_ref())
}

fn resolve_shard_database_urls_from_sources<F>(
    env_var: F,
    table: Option<&toml::Table>,
) -> Vec<(String, String)>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    const MAX_ENV_SHARDS: usize = 64;

    let mut shards: Vec<(String, String)> = table
        .and_then(|table| table.get("database"))
        .and_then(toml::Value::as_table)
        .and_then(|database| database.get("shards"))
        .and_then(toml::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(toml::Value::as_table)
                .filter_map(|shard| {
                    let name = shard.get("name").and_then(toml::Value::as_str)?;
                    let url = shard.get("primary_url").and_then(toml::Value::as_str)?;
                    Some((name.to_owned(), url.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();

    for i in 0..MAX_ENV_SHARDS {
        let name_var = format!("AUTUMN_DATABASE__SHARDS__{i}__NAME");
        let url_var = format!("AUTUMN_DATABASE__SHARDS__{i}__PRIMARY_URL");
        if i >= shards.len() {
            let (Ok(name), Ok(url)) = (env_var(&name_var), env_var(&url_var)) else {
                break;
            };
            shards.push((name, url));
            continue;
        }
        if let Ok(name) = env_var(&name_var) {
            shards[i].0 = name;
        }
        if let Ok(url) = env_var(&url_var) {
            shards[i].1 = url;
        }
    }

    shards
}

/// Resolve the migrations directory (default: `./migrations/`).
fn resolve_migrations_dir() -> String {
    let dir = Path::new(DEFAULT_MIGRATIONS_DIR);
    if !dir.exists() {
        eprintln!("\u{2717} Migrations directory not found: {DEFAULT_MIGRATIONS_DIR}/");
        eprintln!("  Create it with `diesel setup` or `diesel migration generate <name>`.");
        std::process::exit(1);
    }
    DEFAULT_MIGRATIONS_DIR.to_string()
}

/// Check that the `diesel` CLI is installed and available on PATH.
fn check_diesel_cli() {
    match Command::new("diesel").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            eprintln!("  Using {}", version.trim());
        }
        _ => {
            eprintln!("\u{2717} diesel CLI not found on PATH.");
            eprintln!(
                "  Install it with: cargo install diesel_cli --no-default-features --features postgres"
            );
            std::process::exit(1);
        }
    }
}

fn run_framework_migrations(database_url: &str) -> bool {
    eprintln!("  Running pending Autumn framework migrations...\n");

    match run_framework_migrations_inner(database_url, autumn_web::migrate::run_pending) {
        Ok(result) if result.applied.is_empty() => {
            eprintln!("\n\u{2713} Framework migrations are up to date.");
            true
        }
        Ok(result) => {
            for migration in &result.applied {
                eprintln!("  Applied {migration}");
            }
            eprintln!("\n\u{2713} Framework migrations applied successfully.");
            true
        }
        Err(e) => {
            eprintln!("\n\u{2717} Framework migration failed: {e}");
            false
        }
    }
}

fn run_framework_migrations_inner<F>(
    database_url: &str,
    run_pending: F,
) -> Result<MigrationResult, MigrationError>
where
    F: FnOnce(&str, EmbeddedMigrations) -> Result<MigrationResult, MigrationError>,
{
    run_pending(database_url, FRAMEWORK_MIGRATIONS)
}

/// True iff the active profile is `prod` or `production`.
///
/// Reads `AUTUMN_ENV` (preferred) then `AUTUMN_PROFILE` (legacy alias),
/// normalising case and whitespace — same rules as `autumn_web::config`.
fn is_production_profile() -> bool {
    // Treat an empty/whitespace `AUTUMN_ENV` as absent (matching
    // `autumn_web::config`) so the legacy `AUTUMN_PROFILE` is still consulted.
    let read = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let profile = read("AUTUMN_ENV")
        .or_else(|| read("AUTUMN_PROFILE"))
        .unwrap_or_default();
    let normalized = profile.trim().to_lowercase();
    normalized == "prod" || normalized == "production"
}

/// Whether an applied user migration has a usable `down.sql` on disk.
///
/// Returns `false` when the migration is not present locally (`dir` is `None`)
/// or its `down.sql` is missing/empty/comment-only.
fn has_revertable_down_sql(m: &AppliedUserMigration) -> bool {
    m.dir.as_ref().is_some_and(|d| {
        std::fs::read_to_string(d.join("down.sql"))
            .ok()
            .is_some_and(|sql| safety::has_executable_sql(&sql))
    })
}

/// Whether reverting `m` would fail mid-rollback because its `down.sql` runs a
/// `CREATE`/`DROP INDEX CONCURRENTLY` but the migration does not opt out of
/// Diesel's default per-migration transaction via `metadata.toml`.
///
/// `PostgreSQL` rejects `CONCURRENTLY` index operations inside a transaction
/// block, so such a revert fails at execution time. In a multi-step rollback
/// that failure lands *after* earlier (newer) migrations have already been
/// reverted and committed, leaving a partial rollback — so it is surfaced in
/// preflight, before anything is mutated. Migrations missing locally (`dir`
/// is `None`) are reported by the separate `has_revertable_down_sql` preflight.
fn down_sql_concurrent_without_opt_out(m: &AppliedUserMigration) -> bool {
    m.dir.as_ref().is_some_and(|d| {
        std::fs::read_to_string(d.join("down.sql"))
            .ok()
            .is_some_and(|sql| safety::contains_concurrent_index(&sql))
            && !migration_opts_out_of_transaction(d)
    })
}

/// Build the newest-first list of user-migration versions to revert.
///
/// With `--to VERSION`, every applied user migration strictly newer than
/// `VERSION` is reverted (exiting non-zero if `VERSION` is not a currently
/// applied *user* migration). `VERSION` must be a user migration version —
/// framework migrations are forward-only and cannot serve as a boundary.
/// Otherwise the most recently applied `--steps N` (default 1) versions are
/// reverted.
///
/// `applied` is ascending by version, so the newest-first plan is its reverse.
fn build_rollback_plan(args: &DownArgs, applied: &[AppliedUserMigration]) -> Vec<String> {
    let Some(target_version) = args.to.as_deref() else {
        let n = args.steps.unwrap_or(1);
        return applied
            .iter()
            .rev()
            .take(n)
            .map(|m| m.version.clone())
            .collect();
    };

    if !applied.iter().any(|m| m.version == target_version) {
        eprintln!("\u{2717} Version {target_version} is not a currently applied user migration.");
        eprintln!("  Check `autumn migrate status` to see the applied user migrations.");
        std::process::exit(1);
    }
    applied
        .iter()
        .filter(|m| m.version.as_str() > target_version)
        .rev()
        .map(|m| m.version.clone())
        .collect()
}

/// Run `autumn migrate down`.
fn run_down(args: &DownArgs, with_maintenance: bool, target: &MigrateTarget) {
    // 1. Production guard
    if is_production_profile() && !args.yes_i_mean_prod {
        eprintln!("\u{2717} Production profile detected.");
        eprintln!("  Rolling back migrations in production requires explicit confirmation.");
        eprintln!("  Re-run with --yes-i-mean-prod to proceed.");
        std::process::exit(1);
    }

    // 2. Resolve target databases (control + shards / a single shard /
    //    control-only) and the migrations dir. `down` honors --shard /
    //    --control-only exactly like `migrate run`.
    let targets = resolve_targets(target);
    let migrations_dir = resolve_migrations_dir();
    let dir = Path::new(&migrations_dir);

    // Multi-target rollbacks are fail-fast and applied in order, matching the
    // forward `migrate run`: each target is reverted before the next is
    // planned. If a later target fails (a runtime down.sql error, or shards at
    // divergent migration states), earlier targets are already rolled back —
    // re-running `down` then plans from each target's current state. To roll a
    // single database back in isolation, scope the command with --shard /
    // --control-only. (Missing/empty down.sql is caught by preflight before any
    // mutation, since all targets share one migrations dir.)
    //
    // Maintenance mode is a global flag, so enable it at most once (the first
    // time any target actually has work) and disable it after all targets
    // complete. Tracked across targets so an empty plan never toggles it.
    let mut maintenance_enabled = false;
    let mut total_reverted = 0usize;
    let multi = targets.len() > 1;

    for (label, url) in &targets {
        if multi {
            eprintln!("\u{2500}\u{2500} Rolling back {label} \u{2500}\u{2500}");
        }
        match run_down_target(args, url, dir, with_maintenance, &mut maintenance_enabled) {
            Ok(n) => {
                total_reverted += n;
                if multi {
                    eprintln!();
                }
            }
            Err(e) => {
                eprintln!("\n\u{2717} Rollback failed for {label}: {e}");
                if maintenance_enabled {
                    eprintln!(
                        "  \u{26A0}\u{FE0F}  Maintenance mode left ON for safety. Investigate, then run \
                         `autumn maintenance off` to re-open traffic."
                    );
                }
                std::process::exit(1);
            }
        }
    }

    if total_reverted > 0 {
        eprintln!("\n\u{2713} {total_reverted} migration(s) rolled back.");
    }
    if maintenance_enabled {
        disable_maintenance_after_migrate();
    }
}

/// Roll back the planned user migrations on a single target database, under the
/// migration advisory lock. Returns the number of migrations reverted.
///
/// Listing applied migrations, building the plan, and preflighting `down.sql`
/// all happen inside the `plan` closure (under the lock) so the plan cannot go
/// stale between read and execute (e.g. two concurrent `down` runs).
/// `maintenance_enabled` is shared across targets so maintenance mode is
/// enabled at most once, the first time any target has work to do.
fn run_down_target(
    args: &DownArgs,
    database_url: &str,
    dir: &Path,
    with_maintenance: bool,
    maintenance_enabled: &mut bool,
) -> Result<usize, autumn_web::migrate::MigrationError> {
    use autumn_web::migrate::revert_user_migrations_locked;

    revert_user_migrations_locked(
        database_url,
        dir,
        None,
        |applied| {
            let plan = build_rollback_plan(args, applied);
            if plan.is_empty() {
                eprintln!("  \u{2713} Nothing to roll back.");
                return Ok(plan);
            }

            // Preflight: every planned migration must have an executable
            // down.sql. Applied-but-missing-locally migrations (dir = None) are
            // surfaced here by name rather than silently skipped.
            let missing: Vec<&str> = plan
                .iter()
                .filter_map(|version| {
                    let m = applied.iter().find(|m| &m.version == version)?;
                    (!has_revertable_down_sql(m)).then_some(m.name.as_str())
                })
                .collect();
            if !missing.is_empty() {
                eprintln!(
                    "\u{2717} The following migration(s) have no executable down.sql and cannot be reverted:"
                );
                for name in &missing {
                    eprintln!("    \u{2022} {name}");
                }
                eprintln!(
                    "  Add a down.sql to each migration before running `autumn migrate down`."
                );
                std::process::exit(1);
            }

            // Preflight: a down.sql running `CONCURRENTLY` index ops without
            // `run_in_transaction = false` will fail inside Diesel's
            // per-migration transaction. In a multi-step plan that failure lands
            // after earlier (newer) reverts have already committed, so refuse
            // the whole plan up front rather than leave a partial rollback.
            let needs_opt_out: Vec<&str> = plan
                .iter()
                .filter_map(|version| {
                    let m = applied.iter().find(|m| &m.version == version)?;
                    down_sql_concurrent_without_opt_out(m).then_some(m.name.as_str())
                })
                .collect();
            if !needs_opt_out.is_empty() {
                eprintln!(
                    "\u{2717} The following migration(s) revert a `CONCURRENTLY` index but do not set \
                     `run_in_transaction = false` and cannot be reverted safely:"
                );
                for name in &needs_opt_out {
                    eprintln!("    \u{2022} {name}");
                }
                eprintln!(
                    "  `PostgreSQL` rejects `CONCURRENTLY` index operations inside a transaction, so \
                     the revert would fail partway through. Add `run_in_transaction = false` to each \
                     migration's metadata.toml before running `autumn migrate down`."
                );
                std::process::exit(1);
            }

            // Preflight passed: enable maintenance mode (if requested and not
            // already enabled for an earlier target) before we mutate schema,
            // then stream the rollback.
            if with_maintenance && !*maintenance_enabled {
                enable_maintenance_for_migrate();
                *maintenance_enabled = true;
            }
            eprintln!("  Rolling back {} migration(s)...\n", plan.len());
            Ok(plan)
        },
        |r| {
            eprintln!(
                "  \u{2713} Rolled back {}  ({}ms)",
                r.name,
                r.duration.as_millis()
            );
        },
    )
}

/// Show rollback availability for all applied user migrations.
fn show_rollback_availability(database_url: &str, migrations_dir: &str) {
    use autumn_web::migrate::applied_user_migrations;

    let dir = Path::new(migrations_dir);
    let applied = match applied_user_migrations(database_url, dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  \u{26A0}\u{FE0F}  Could not check rollback availability: {e}");
            return;
        }
    };

    eprintln!("  Rollback availability (user migrations):\n");

    if applied.is_empty() {
        eprintln!("  No applied user migrations.");
        return;
    }

    for m in &applied {
        if m.dir.is_none() {
            eprintln!(
                "  \u{2717} {}  (applied but missing locally — not revertable)",
                m.name
            );
        } else if has_revertable_down_sql(m) {
            eprintln!("  \u{2713} {}", m.name);
        } else {
            eprintln!(
                "  \u{2717} {}  (no executable down.sql — not revertable)",
                m.name
            );
        }
    }
    eprintln!();
}

/// Show migration status via `diesel migration pending`.
fn show_status(database_url: &str, migrations_dir: &str) {
    eprintln!("  Checking migration status...\n");
    show_diesel_migration_status(database_url, Path::new(migrations_dir));
}

fn show_framework_status(database_url: &str) {
    eprintln!("  Checking Autumn framework migration status...\n");

    match pending_framework_migrations_inner(database_url, autumn_web::migrate::pending_migrations)
    {
        Ok(pending) if pending.is_empty() => {
            eprintln!("  Framework migrations are up to date.");
        }
        Ok(pending) => {
            eprintln!("  Pending Autumn framework migrations:");
            for migration in pending {
                eprintln!("    {migration}");
            }
        }
        Err(e) => {
            eprintln!("\u{2717} Failed to check framework migration status: {e}");
            std::process::exit(1);
        }
    }
}

fn pending_framework_migrations_inner<F>(
    database_url: &str,
    pending_migrations: F,
) -> Result<Vec<String>, MigrationError>
where
    F: FnOnce(&str, EmbeddedMigrations) -> Result<Vec<String>, MigrationError>,
{
    pending_migrations(database_url, FRAMEWORK_MIGRATIONS)
}

fn show_diesel_migration_status(database_url: &str, migrations_dir: &Path) {
    // `diesel migration list` shows all migrations and their status
    let status = Command::new("diesel")
        .args(["migration", "list", "--migration-dir"])
        .arg(migrations_dir)
        .env("DATABASE_URL", database_url)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            eprintln!(
                "\n\u{2717} Failed to check migration status for {}.",
                migrations_dir.display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "\u{2717} Failed to run diesel migration list for {}: {e}",
                migrations_dir.display()
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Advisory-lock API accessibility ───────────────────────────────────

    #[test]
    fn migration_lock_key_exported_from_autumn_web() {
        let key = autumn_web::migrate::MIGRATION_ADVISORY_LOCK_KEY;
        assert!(key > 0, "lock key must be a positive i64");
    }

    #[test]
    fn default_lock_wait_timeout_is_sixty_seconds() {
        let timeout = autumn_web::migrate::DEFAULT_LOCK_WAIT_TIMEOUT;
        assert_eq!(timeout.as_secs(), 60);
    }

    #[test]
    fn hold_migration_lock_returns_connection_error_on_bad_url() {
        let result = autumn_web::migrate::hold_migration_lock(
            "postgres://invalid_user:invalid_password@0.0.0.0:1/invalid_db",
            std::time::Duration::from_secs(1),
        );
        assert!(result.is_err());
        assert!(
            matches!(
                result.unwrap_err(),
                autumn_web::migrate::MigrationError::Connection(_)
            ),
            "unreachable host must produce Connection error"
        );
    }

    // ── Existing tests ────────────────────────────────────────────────────

    #[test]
    fn migrate_action_eq() {
        assert_eq!(MigrateAction::Run, MigrateAction::Run);
        assert_eq!(MigrateAction::Status, MigrateAction::Status);
        assert_eq!(MigrateAction::Check, MigrateAction::Check);
        assert_ne!(MigrateAction::Run, MigrateAction::Status);
        assert_ne!(MigrateAction::Run, MigrateAction::Check);
    }

    // ── check_migrations_in_dir ────────────────────────────────────────────

    fn write_migration(dir: &std::path::Path, name: &str, up_sql: &str) {
        let migration_dir = dir.join(name);
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(migration_dir.join("up.sql"), up_sql).unwrap();
        std::fs::write(migration_dir.join("down.sql"), "").unwrap();
    }

    fn write_migration_with_down(dir: &std::path::Path, name: &str, up_sql: &str, down_sql: &str) {
        let migration_dir = dir.join(name);
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(migration_dir.join("up.sql"), up_sql).unwrap();
        std::fs::write(migration_dir.join("down.sql"), down_sql).unwrap();
    }

    #[test]
    fn check_empty_migrations_dir_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn check_safe_migration_produces_no_findings() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260101000000_create_posts",
            "CREATE TABLE posts (id BIGSERIAL PRIMARY KEY, title TEXT NOT NULL);",
        );

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "20260101000000_create_posts");
        assert!(
            results[0].up.is_empty(),
            "CREATE TABLE should produce no up findings"
        );
        assert!(
            results[0].down.is_empty(),
            "empty down.sql should produce no findings"
        );
    }

    #[test]
    fn check_down_sql_findings_are_included_in_report() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration_with_down(
            tmp.path(),
            "20260101000000_add_column",
            "ALTER TABLE posts ADD COLUMN body TEXT;",
            "ALTER TABLE posts DROP COLUMN body;",
        );

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].up.is_empty(), "ADD COLUMN should be safe in up");
        assert_eq!(
            results[0].down.len(),
            1,
            "DROP COLUMN in down.sql should have 1 finding"
        );
        assert_eq!(results[0].down[0].risk, safety::RiskLevel::Destructive);
    }

    #[test]
    fn check_down_sql_absent_produces_empty_findings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260101000000_create_posts");
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(
            migration_dir.join("up.sql"),
            "CREATE TABLE posts (id BIGSERIAL);",
        )
        .unwrap();

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            results[0].down.is_empty(),
            "absent down.sql should produce empty findings"
        );
    }

    #[test]
    fn rolling_deploy_gate_ignores_destructive_down_sql() {
        // A forward-safe migration whose down.sql is inherently destructive
        // (ADD COLUMN up / DROP COLUMN down) must NOT block the forward deploy.
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration_with_down(
            tmp.path(),
            "20260101000000_add_column",
            "ALTER TABLE posts ADD COLUMN body TEXT;",
            "ALTER TABLE posts DROP COLUMN body;",
        );
        let reports = check_migrations_in_dir(tmp.path()).unwrap();
        assert!(
            !reports[0].down.is_empty(),
            "precondition: down.sql has a destructive finding"
        );
        assert!(
            !rolling_deploy_blocked(&reports),
            "destructive down.sql must not block a forward rolling deploy"
        );
    }

    #[test]
    fn rolling_deploy_gate_blocks_on_unsafe_up_sql() {
        // An unsafe forward operation (DROP COLUMN in up.sql) must block.
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration_with_down(
            tmp.path(),
            "20260101000000_drop_column",
            "ALTER TABLE posts DROP COLUMN body;",
            "ALTER TABLE posts ADD COLUMN body TEXT;",
        );
        let reports = check_migrations_in_dir(tmp.path()).unwrap();
        assert!(
            rolling_deploy_blocked(&reports),
            "unsafe up.sql must block the forward rolling deploy"
        );
    }

    #[test]
    fn check_destructive_migration_produces_findings() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260102000000_remove_body_from_posts",
            "ALTER TABLE posts DROP COLUMN body;",
        );

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].up.len(), 1);
        assert_eq!(results[0].up[0].risk, safety::RiskLevel::Destructive);
    }

    #[test]
    fn check_results_are_sorted_by_migration_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration(tmp.path(), "20260103000000_third", "SELECT 1;");
        write_migration(tmp.path(), "20260101000000_first", "SELECT 1;");
        write_migration(tmp.path(), "20260102000000_second", "SELECT 1;");

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        let names: Vec<_> = results.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "20260101000000_first",
                "20260102000000_second",
                "20260103000000_third"
            ]
        );
    }

    #[test]
    fn check_directories_without_up_sql_are_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("incomplete_migration")).unwrap();
        write_migration(tmp.path(), "20260101000000_valid", "SELECT 1;");

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "20260101000000_valid");
    }

    #[test]
    fn check_multiple_migrations_reports_each() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260101000000_create_posts",
            "CREATE TABLE posts (id BIGSERIAL PRIMARY KEY);",
        );
        write_migration(
            tmp.path(),
            "20260102000000_remove_body",
            "ALTER TABLE posts DROP COLUMN body;",
        );

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].up.is_empty(), "first migration should be safe");
        assert!(
            !results[1].up.is_empty(),
            "second migration should have findings"
        );
    }

    #[test]
    fn check_non_concurrent_index_is_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration(
            tmp.path(),
            "20260103000000_add_index",
            "CREATE INDEX idx_posts_title ON posts (title);",
        );

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results[0].up.len(), 1);
        assert_eq!(
            results[0].up[0].risk,
            safety::RiskLevel::PotentiallyBlocking
        );
    }

    // ── is_production_profile ─────────────────────────────────────────────────

    #[test]
    fn is_production_profile_detects_prod() {
        temp_env::with_var("AUTUMN_ENV", Some("prod"), || {
            assert!(is_production_profile());
        });
    }

    #[test]
    fn is_production_profile_detects_production() {
        temp_env::with_var("AUTUMN_ENV", Some("production"), || {
            assert!(is_production_profile());
        });
    }

    #[test]
    fn is_production_profile_case_insensitive() {
        temp_env::with_var("AUTUMN_ENV", Some("PROD"), || {
            assert!(is_production_profile());
        });
    }

    #[test]
    fn is_production_profile_false_for_dev() {
        temp_env::with_var("AUTUMN_ENV", Some("dev"), || {
            assert!(!is_production_profile());
        });
    }

    #[test]
    fn is_production_profile_reads_autumn_profile_legacy() {
        temp_env::with_vars(
            [("AUTUMN_ENV", None), ("AUTUMN_PROFILE", Some("production"))],
            || {
                assert!(is_production_profile());
            },
        );
    }

    #[test]
    fn is_production_profile_empty_autumn_env_falls_back_to_legacy() {
        // An empty/whitespace AUTUMN_ENV must be treated as absent so the
        // legacy AUTUMN_PROFILE still triggers the production guard.
        temp_env::with_vars(
            [
                ("AUTUMN_ENV", Some("   ")),
                ("AUTUMN_PROFILE", Some("prod")),
            ],
            || {
                assert!(is_production_profile());
            },
        );
    }

    #[test]
    fn default_migrations_dir_is_migrations() {
        assert_eq!(DEFAULT_MIGRATIONS_DIR, "migrations");
    }

    // ── rollback planning ────────────────────────────────────────────────────

    fn applied(version: &str) -> AppliedUserMigration {
        AppliedUserMigration {
            version: version.to_string(),
            name: format!("{version}_m"),
            dir: Some(std::path::PathBuf::from(format!("migrations/{version}_m"))),
        }
    }

    #[test]
    fn build_rollback_plan_default_reverts_single_newest() {
        // `applied` is ascending; default plan reverts only the newest.
        let applied = [applied("20260101000000"), applied("20260102000000")];
        let args = DownArgs {
            steps: None,
            to: None,
            yes_i_mean_prod: false,
        };
        assert_eq!(build_rollback_plan(&args, &applied), ["20260102000000"]);
    }

    #[test]
    fn build_rollback_plan_steps_reverts_n_newest_first() {
        let applied = [
            applied("20260101000000"),
            applied("20260102000000"),
            applied("20260103000000"),
        ];
        let args = DownArgs {
            steps: Some(2),
            to: None,
            yes_i_mean_prod: false,
        };
        // Newest-first so dependent migrations revert before their dependencies.
        assert_eq!(
            build_rollback_plan(&args, &applied),
            ["20260103000000", "20260102000000"]
        );
    }

    #[test]
    fn build_rollback_plan_to_reverts_strictly_newer_newest_first() {
        let applied = [
            applied("20260101000000"),
            applied("20260102000000"),
            applied("20260103000000"),
        ];
        let args = DownArgs {
            steps: None,
            to: Some("20260101000000".to_string()),
            yes_i_mean_prod: false,
        };
        assert_eq!(
            build_rollback_plan(&args, &applied),
            ["20260103000000", "20260102000000"]
        );
    }

    #[test]
    fn has_revertable_down_sql_false_when_missing_locally() {
        let m = AppliedUserMigration {
            version: "20260101000000".to_string(),
            name: "20260101000000".to_string(),
            dir: None,
        };
        assert!(!has_revertable_down_sql(&m));
    }

    #[test]
    fn has_revertable_down_sql_true_for_executable_down() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("20260101000000_create_posts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("down.sql"), "DROP TABLE posts;").unwrap();
        let m = AppliedUserMigration {
            version: "20260101000000".to_string(),
            name: "20260101000000_create_posts".to_string(),
            dir: Some(dir),
        };
        assert!(has_revertable_down_sql(&m));
    }

    #[test]
    fn has_revertable_down_sql_false_for_comment_only_down() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("20260101000000_create_posts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("down.sql"), "-- nothing to do\n").unwrap();
        let m = AppliedUserMigration {
            version: "20260101000000".to_string(),
            name: "20260101000000_create_posts".to_string(),
            dir: Some(dir),
        };
        assert!(!has_revertable_down_sql(&m));
    }

    /// Build an `AppliedUserMigration` backed by a temp dir containing the given
    /// `down.sql` (and optional `metadata.toml`). Returns the migration plus the
    /// `TempDir` guard, which must be kept alive for the files to exist.
    fn migration_with_down(
        down_sql: &str,
        metadata: Option<&str>,
    ) -> (AppliedUserMigration, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("20260101000000_add_index");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("down.sql"), down_sql).unwrap();
        if let Some(meta) = metadata {
            std::fs::write(dir.join("metadata.toml"), meta).unwrap();
        }
        let m = AppliedUserMigration {
            version: "20260101000000".to_string(),
            name: "20260101000000_add_index".to_string(),
            dir: Some(dir),
        };
        (m, tmp)
    }

    #[test]
    fn down_concurrent_without_metadata_needs_opt_out() {
        let (m, _tmp) = migration_with_down("DROP INDEX CONCURRENTLY idx_posts_title;", None);
        assert!(down_sql_concurrent_without_opt_out(&m));
    }

    #[test]
    fn down_concurrent_with_opt_out_is_safe() {
        let (m, _tmp) = migration_with_down(
            "DROP INDEX CONCURRENTLY idx_posts_title;",
            Some("run_in_transaction = false\n"),
        );
        assert!(!down_sql_concurrent_without_opt_out(&m));
    }

    #[test]
    fn down_without_concurrent_index_is_safe() {
        let (m, _tmp) = migration_with_down("DROP TABLE posts;", None);
        assert!(!down_sql_concurrent_without_opt_out(&m));
    }

    #[test]
    fn down_concurrent_missing_locally_is_not_flagged_here() {
        // Reported by the separate missing-down.sql preflight, not this one.
        let m = AppliedUserMigration {
            version: "20260101000000".to_string(),
            name: "20260101000000_add_index".to_string(),
            dir: None,
        };
        assert!(!down_sql_concurrent_without_opt_out(&m));
    }

    #[test]
    fn framework_api_token_migrations_run_from_embedded_set() {
        let mut called = false;

        let result = run_framework_migrations_inner(
            "postgres://primary/app",
            |database_url, embedded_migrations| {
                assert_eq!(database_url, "postgres://primary/app");
                let _ = embedded_migrations;
                called = true;
                Ok(autumn_web::migrate::MigrationResult {
                    applied: vec!["20260512000000_create_api_tokens".to_string()],
                })
            },
        )
        .unwrap();

        assert!(called);
        assert_eq!(
            result.applied,
            vec!["20260512000000_create_api_tokens".to_string()]
        );
    }

    #[test]
    fn framework_api_token_status_uses_embedded_set() {
        let mut called = false;

        let pending = pending_framework_migrations_inner(
            "postgres://primary/app",
            |database_url, embedded_migrations| {
                assert_eq!(database_url, "postgres://primary/app");
                let _ = embedded_migrations;
                called = true;
                Ok(vec!["20260512000000_create_api_tokens".to_string()])
            },
        )
        .unwrap();

        assert!(called);
        assert_eq!(
            pending,
            vec!["20260512000000_create_api_tokens".to_string()]
        );
    }

    #[test]
    fn embedded_framework_migrations_include_durable_hook_queue() {
        use autumn_web::reexports::diesel::migration::{Migration, MigrationSource};
        use autumn_web::reexports::diesel::pg::Pg;

        let migrations: Vec<Box<dyn Migration<Pg>>> = autumn_web::migrate::FRAMEWORK_MIGRATIONS
            .migrations()
            .unwrap();
        let names: Vec<_> = migrations
            .iter()
            .map(|migration| migration.name().to_string())
            .collect();

        assert!(
            names
                .iter()
                .any(|name| name == "20260512000000_create_api_tokens"),
            "framework migrations must include the timestamped API token schema migration: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|name| name == "20260515000000_create_repository_commit_hook_queue"),
            "framework migrations must include the durable repository commit hook queue: {names:?}"
        );
    }

    #[test]
    fn resolve_database_url_from_env() {
        // AUTUMN_DATABASE__URL takes priority
        let env_var = |key: &str| -> Result<String, std::env::VarError> {
            if key == "AUTUMN_DATABASE__URL" {
                Ok("postgres://test:5432/mydb".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        };
        let url = resolve_database_url_with_env(env_var);
        assert_eq!(url, "postgres://test:5432/mydb");
    }

    #[test]
    fn resolve_database_url_from_database_url_env() {
        // Make sure AUTUMN_DATABASE__URL is not set, but DATABASE_URL is
        let env_var = |key: &str| -> Result<String, std::env::VarError> {
            if key == "DATABASE_URL" {
                Ok("postgres://fallback:5432/db".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        };
        let url = resolve_database_url_with_env(env_var);
        assert_eq!(url, "postgres://fallback:5432/db");
    }

    #[test]
    fn database_topology_primary_env_wins_for_migrations() {
        let env_var = |key: &str| -> Result<String, std::env::VarError> {
            match key {
                "AUTUMN_DATABASE__PRIMARY_URL" => Ok("postgres://primary:5432/app".to_string()),
                "AUTUMN_DATABASE__URL" => Ok("postgres://legacy:5432/app".to_string()),
                "DATABASE_URL" => Ok("postgres://database-url:5432/app".to_string()),
                _ => Err(std::env::VarError::NotPresent),
            }
        };

        let url = resolve_primary_database_url_from_sources(env_var, None).unwrap();

        assert_eq!(url, "postgres://primary:5432/app");
    }

    #[test]
    fn database_topology_toml_primary_wins_over_legacy_url() {
        let table = toml::from_str::<toml::Table>(
            r#"
[database]
url = "postgres://legacy:5432/app"
primary_url = "postgres://primary:5432/app"
replica_url = "postgres://replica:5432/app"
"#,
        )
        .unwrap();
        let env_var = |_key: &str| -> Result<String, std::env::VarError> {
            Err(std::env::VarError::NotPresent)
        };

        let url = resolve_primary_database_url_from_sources(env_var, Some(&table)).unwrap();

        assert_eq!(url, "postgres://primary:5432/app");
    }

    // ── build_targets ──────────────────────────────────────────────────────

    fn two_shards() -> Vec<(String, String)> {
        vec![
            ("shard0".to_owned(), "postgres://s0/app".to_owned()),
            ("shard1".to_owned(), "postgres://s1/app".to_owned()),
        ]
    }

    #[test]
    fn build_targets_all_orders_control_first_then_shards() {
        let targets = build_targets(
            Some("postgres://control/app".to_owned()),
            two_shards(),
            &MigrateTarget::All,
        )
        .unwrap();

        let labels: Vec<&str> = targets.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["control", "shard:shard0", "shard:shard1"]);
        assert_eq!(targets[0].1, "postgres://control/app");
    }

    #[test]
    fn build_targets_all_supports_shard_only_deployments() {
        let targets = build_targets(None, two_shards(), &MigrateTarget::All).unwrap();
        let labels: Vec<&str> = targets.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["shard:shard0", "shard:shard1"]);
    }

    #[test]
    fn build_targets_all_errors_with_no_databases_at_all() {
        let error = build_targets(None, Vec::new(), &MigrateTarget::All).unwrap_err();
        assert!(error.contains("No database URL"));
    }

    #[test]
    fn build_targets_control_only_requires_control() {
        let targets = build_targets(
            Some("postgres://control/app".to_owned()),
            two_shards(),
            &MigrateTarget::ControlOnly,
        )
        .unwrap();
        assert_eq!(
            targets,
            vec![("control".to_owned(), "postgres://control/app".to_owned())]
        );

        let error = build_targets(None, two_shards(), &MigrateTarget::ControlOnly).unwrap_err();
        assert!(error.contains("control database"));
    }

    #[test]
    fn build_targets_selects_single_shard_by_name() {
        let targets = build_targets(
            None,
            two_shards(),
            &MigrateTarget::Shard("shard1".to_owned()),
        )
        .unwrap();
        assert_eq!(
            targets,
            vec![("shard:shard1".to_owned(), "postgres://s1/app".to_owned())]
        );
    }

    #[test]
    fn build_targets_unknown_shard_lists_known_names() {
        let error = build_targets(None, two_shards(), &MigrateTarget::Shard("nope".to_owned()))
            .unwrap_err();
        assert!(error.contains("Unknown shard"));
        assert!(error.contains("shard0, shard1"));

        let error =
            build_targets(None, Vec::new(), &MigrateTarget::Shard("nope".to_owned())).unwrap_err();
        assert!(error.contains("No [[database.shards]] entries"));
    }

    // ── resolve_shard_database_urls ────────────────────────────────────────

    fn no_env(_key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }

    #[test]
    fn shard_urls_resolve_from_toml_in_declaration_order() {
        let table = toml::from_str::<toml::Table>(
            r#"
[database]
primary_url = "postgres://control:5432/app"

[[database.shards]]
name = "shard0"
primary_url = "postgres://shard0:5432/app"

[[database.shards]]
name = "shard1"
primary_url = "postgres://shard1:5432/app"
"#,
        )
        .unwrap();

        let shards = resolve_shard_database_urls_from_sources(no_env, Some(&table));

        assert_eq!(
            shards,
            vec![
                ("shard0".to_owned(), "postgres://shard0:5432/app".to_owned()),
                ("shard1".to_owned(), "postgres://shard1:5432/app".to_owned()),
            ]
        );
    }

    #[test]
    fn shard_urls_env_overrides_and_appends() {
        let table = toml::from_str::<toml::Table>(
            r#"
[[database.shards]]
name = "shard0"
primary_url = "postgres://toml:5432/app"
"#,
        )
        .unwrap();
        let env_var = |key: &str| -> Result<String, std::env::VarError> {
            match key {
                "AUTUMN_DATABASE__SHARDS__0__PRIMARY_URL" => {
                    Ok("postgres://env:5432/app".to_owned())
                }
                "AUTUMN_DATABASE__SHARDS__1__NAME" => Ok("shard1".to_owned()),
                "AUTUMN_DATABASE__SHARDS__1__PRIMARY_URL" => {
                    Ok("postgres://env1:5432/app".to_owned())
                }
                _ => Err(std::env::VarError::NotPresent),
            }
        };

        let shards = resolve_shard_database_urls_from_sources(env_var, Some(&table));

        assert_eq!(
            shards,
            vec![
                ("shard0".to_owned(), "postgres://env:5432/app".to_owned()),
                ("shard1".to_owned(), "postgres://env1:5432/app".to_owned()),
            ]
        );
    }

    #[test]
    fn shard_urls_empty_without_shards() {
        assert!(resolve_shard_database_urls_from_sources(no_env, None).is_empty());
    }

    // ── check_concurrent_index_transaction_opt_out ────────────────────────

    #[test]
    fn concurrent_index_without_metadata_toml_is_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();

        let sql = "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, safety::RiskLevel::PotentiallyBlocking);
        assert!(
            findings[0].operation.contains("CONCURRENTLY"),
            "finding should mention CONCURRENTLY"
        );
        assert!(
            findings[0]
                .next_action
                .contains("run_in_transaction = false"),
            "next_action should guide user to metadata.toml"
        );
    }

    #[test]
    fn concurrent_index_with_run_in_transaction_false_is_not_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(
            migration_dir.join("metadata.toml"),
            "run_in_transaction = false\n",
        )
        .unwrap();

        let sql = "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert!(
            findings.is_empty(),
            "correctly opted-out CONCURRENTLY should produce no additional findings"
        );
    }

    #[test]
    fn concurrent_index_with_run_in_transaction_false_no_spaces_is_not_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(
            migration_dir.join("metadata.toml"),
            "run_in_transaction=false\n",
        )
        .unwrap();

        let sql = "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert!(
            findings.is_empty(),
            "TOML `run_in_transaction=false` (no spaces) should also suppress the finding"
        );
    }

    #[test]
    fn concurrent_index_with_commented_out_flag_is_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(
            migration_dir.join("metadata.toml"),
            "# run_in_transaction = false\n",
        )
        .unwrap();

        let sql = "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert_eq!(
            findings.len(),
            1,
            "a commented-out opt-out should NOT suppress the finding"
        );
        assert_eq!(findings[0].risk, safety::RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn concurrent_index_with_metadata_toml_missing_flag_is_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(
            migration_dir.join("metadata.toml"),
            "# Diesel migration metadata\n",
        )
        .unwrap();

        let sql = "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, safety::RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn non_concurrent_index_is_not_flagged_by_opt_out_check() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();

        let sql = "CREATE INDEX idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert!(
            findings.is_empty(),
            "non-CONCURRENTLY index should not be flagged by opt-out check"
        );
    }

    #[test]
    fn concurrent_index_in_sql_comment_is_not_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();

        // The concurrent index is only mentioned in a comment; the actual
        // statement is a plain (non-concurrent) CREATE INDEX.
        let sql = "-- TODO: switch to CREATE INDEX CONCURRENTLY once traffic drops\n\
                   CREATE INDEX idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert!(
            findings.is_empty(),
            "a CONCURRENTLY reference inside a SQL comment must not trigger the opt-out check"
        );
    }

    #[test]
    fn concurrent_unique_index_without_metadata_toml_is_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_unique_index");
        std::fs::create_dir_all(&migration_dir).unwrap();

        let sql = "CREATE UNIQUE INDEX CONCURRENTLY idx_posts_slug ON posts (slug);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, safety::RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn concurrent_index_multiline_without_metadata_toml_is_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();

        let sql = "CREATE INDEX\n  CONCURRENTLY idx_posts_title ON posts (title);";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert_eq!(
            findings.len(),
            1,
            "multi-line CREATE INDEX CONCURRENTLY should be flagged when metadata.toml is absent"
        );
        assert_eq!(findings[0].risk, safety::RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn check_migrations_in_dir_flags_concurrent_index_without_metadata() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(
            migration_dir.join("up.sql"),
            "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);",
        )
        .unwrap();
        std::fs::write(migration_dir.join("down.sql"), "").unwrap();

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            results[0]
                .up
                .iter()
                .any(|f| f.operation.contains("CONCURRENTLY")),
            "missing metadata.toml should produce a CONCURRENTLY finding"
        );
    }

    #[test]
    fn drop_index_concurrently_without_metadata_toml_is_flagged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_drop_index");
        std::fs::create_dir_all(&migration_dir).unwrap();

        let sql = "DROP INDEX CONCURRENTLY idx_posts_title;";
        let mut findings = Vec::new();
        check_concurrent_index_transaction_opt_out(sql, &migration_dir, &mut findings);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, safety::RiskLevel::PotentiallyBlocking);
        assert!(
            findings[0].operation.contains("CONCURRENTLY"),
            "finding should mention CONCURRENTLY"
        );
    }

    #[test]
    fn check_migrations_in_dir_concurrent_index_with_metadata_is_safe() {
        let tmp = tempfile::TempDir::new().unwrap();
        let migration_dir = tmp.path().join("20260104000000_add_index");
        std::fs::create_dir_all(&migration_dir).unwrap();
        std::fs::write(
            migration_dir.join("up.sql"),
            "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);",
        )
        .unwrap();
        std::fs::write(migration_dir.join("down.sql"), "").unwrap();
        std::fs::write(
            migration_dir.join("metadata.toml"),
            "run_in_transaction = false\n",
        )
        .unwrap();

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            !results[0]
                .up
                .iter()
                .any(|f| f.operation.contains("CONCURRENTLY")),
            "opted-out CONCURRENTLY should not produce a transaction opt-out finding"
        );
    }
}
