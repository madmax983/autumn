//! `autumn migrate` -- run or inspect Diesel database migrations.
//!
//! Because the CLI cannot embed the user's application-specific migrations
//! (they live in the user's crate via `embed_migrations!`), this module
//! delegates to the `diesel` CLI tool. It reads the primary/write database URL
//! from `autumn.toml` + environment variables, then shells out to
//! `diesel migration run` or `diesel migration pending`.
//!
//! Framework-owned migrations that must be runnable in production are applied
//! through Autumn's embedded migration harness.
//!
//! The user's application binary is the canonical way to run embedded
//! migrations (via `.migrations()` on `AppBuilder`). This CLI command
//! is a convenience wrapper for explicit migration management.

pub mod safety;

use std::path::Path;
use std::process::Command;

use autumn_web::migrate::{
    EmbeddedMigrations, FRAMEWORK_MIGRATIONS, MigrationError, MigrationResult,
};

/// Default directory containing Diesel migration files.
const DEFAULT_MIGRATIONS_DIR: &str = "migrations";

/// Subcommands for `autumn migrate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateAction {
    /// Run all pending migrations.
    Run,
    /// Show migration status (pending / applied).
    Status,
    /// Preflight safety check — classifies all migration SQL files and returns
    /// a non-zero exit code if any unsafe or unclassified operations are found.
    Check,
}

/// Run the migrate command.
pub fn run(action: MigrateAction, with_maintenance: bool) {
    eprintln!("\u{1F342} autumn migrate\n");

    if action == MigrateAction::Check {
        let migrations_dir = resolve_migrations_dir();
        run_safety_check(&migrations_dir);
        return;
    }

    // 1. Resolve database URL from autumn.toml + env
    let database_url = resolve_database_url();

    // 2. Resolve migrations directory
    let migrations_dir = resolve_migrations_dir();

    // 3. Check that diesel CLI is available
    check_diesel_cli();

    // 4. Enable maintenance mode if requested
    if with_maintenance && action == MigrateAction::Run {
        enable_maintenance_for_migrate();
    }

    // 5. Execute the appropriate diesel command
    match action {
        MigrateAction::Run => {
            run_migrations_with_maintenance(&database_url, &migrations_dir, with_maintenance);
        }
        MigrateAction::Status => {
            show_status(&database_url, &migrations_dir);
            show_framework_status(&database_url);
        }
        MigrateAction::Check => unreachable!("handled above"),
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

fn run_migrations_with_maintenance(
    database_url: &str,
    migrations_dir: &str,
    with_maintenance: bool,
) {
    use autumn_web::migrate::{DEFAULT_LOCK_WAIT_TIMEOUT, hold_migration_lock};

    // Acquire the Postgres advisory lock before reading the pending-migration
    // list.  This serializes concurrent callers (rolling-deploy replicas or
    // parallel `autumn migrate run` invocations): only one process runs
    // migrations at a time; the rest wait and then find no pending work.
    // The lock is released when `_lock_guard` drops (end of this function or
    // process exit — both are safe because PostgreSQL releases session-level
    // advisory locks on connection close).
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
    let _lock_guard =
        hold_migration_lock(database_url, DEFAULT_LOCK_WAIT_TIMEOUT).unwrap_or_else(|e| {
            eprintln!("\u{274C} Failed to acquire migration lock: {e}");
            if with_maintenance {
                eprintln!();
                eprintln!(
                    "  \u{26A0}\u{FE0F}  Lock acquisition failed — maintenance mode left ON for safety."
                );
                eprintln!("      Fix the blocking migration then run `autumn migrate` to retry.");
                eprintln!("      Run `autumn maintenance off` to re-open traffic manually.");
            }
            std::process::exit(1);
        });

    eprintln!("  Running pending migrations...\n");
    let dir = std::path::Path::new(migrations_dir);
    let status = Command::new("diesel")
        .args(["migration", "run", "--migration-dir"])
        .arg(dir)
        .env("DATABASE_URL", database_url)
        .status();

    let success = match status {
        Ok(s) if s.success() => {
            eprintln!("\n\u{2713} Migrations applied successfully.");
            true
        }
        Ok(_) => {
            eprintln!(
                "\n\u{274C} Migration failed in {}. Check the error output above.",
                dir.display()
            );
            false
        }
        Err(e) => {
            eprintln!("\u{274C} Failed to run diesel migration run: {e}");
            false
        }
    };

    if success {
        run_framework_migrations(database_url);
        if with_maintenance {
            // Only disable maintenance when everything succeeded
            disable_maintenance_after_migrate();
        }
    } else {
        if with_maintenance {
            eprintln!();
            eprintln!(
                "  \u{26A0}\u{FE0F}  Migration failed — maintenance mode left ON for safety."
            );
            eprintln!("      Fix the migration then run `autumn migrate` to retry.");
            eprintln!("      Run `autumn maintenance off` to re-open traffic manually.");
        }
        std::process::exit(1);
    }
}

/// Run the migration safety preflight check against all SQL files in `migrations_dir`.
///
/// Prints a human-readable report to stderr and exits with code 1 if any
/// unsafe or potentially-blocking operations are detected.
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

    for (name, findings) in &reports {
        if safety::is_safe(findings) {
            eprintln!("  \u{2713} {name}  [safe]");
        } else {
            eprintln!("  \u{2717} {name}");
            for f in findings {
                eprintln!("      \u{2022} {} [{}]", f.operation, f.risk);
                eprintln!("        Why:  {}", f.why);
                eprintln!("        Next: {}", f.next_action);
            }
        }
    }

    let any_unsafe = reports
        .iter()
        .any(|(_, findings)| safety::has_unsafe_findings(findings));

    eprintln!();
    if any_unsafe {
        eprintln!(
            "\u{2717} One or more migrations contain operations that are unsafe for a live \
             rolling deploy."
        );
        eprintln!("  Review the findings above, apply the expand/contract pattern where needed,");
        eprintln!("  or coordinate a maintenance window before deploying these migrations.");
        std::process::exit(1);
    } else {
        eprintln!("\u{2713} All {total} migration(s) are safe for a rolling deploy.");
    }
}

/// Read every migration directory in `dir`, classify its `up.sql`, and return
/// a sorted list of `(migration_name, findings)` pairs.
///
/// Migration directories that have no `up.sql` are silently skipped.
pub fn check_migrations_in_dir(
    dir: &Path,
) -> Result<Vec<(String, Vec<safety::SafetyFinding>)>, String> {
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
        let sql = std::fs::read_to_string(&up_sql_path)
            .map_err(|e| format!("cannot read {}: {e}", up_sql_path.display()))?;
        let mut findings = safety::classify_sql(&sql);
        check_concurrent_index_transaction_opt_out(&sql, &entry.path(), &mut findings);
        results.push((migration_name, findings));
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

    let metadata_path = migration_dir.join("metadata.toml");
    let opted_out = std::fs::read_to_string(&metadata_path)
        .ok()
        .and_then(|content| toml::from_str::<toml::Table>(&content).ok())
        .and_then(|table| {
            table
                .get("run_in_transaction")
                .and_then(toml::Value::as_bool)
        })
        .is_some_and(|v| !v);

    if !opted_out {
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

fn run_framework_migrations(database_url: &str) {
    eprintln!("  Running pending Autumn framework migrations...\n");

    match run_framework_migrations_inner(database_url, autumn_web::migrate::run_pending) {
        Ok(result) if result.applied.is_empty() => {
            eprintln!("\n\u{2713} Framework migrations are up to date.");
        }
        Ok(result) => {
            for migration in &result.applied {
                eprintln!("  Applied {migration}");
            }
            eprintln!("\n\u{2713} Framework migrations applied successfully.");
        }
        Err(e) => {
            eprintln!("\n\u{2717} Framework migration failed: {e}");
            std::process::exit(1);
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
        let (name, findings) = &results[0];
        assert_eq!(name, "20260101000000_create_posts");
        assert!(
            findings.is_empty(),
            "CREATE TABLE should produce no findings"
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
        let (_, findings) = &results[0];
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, safety::RiskLevel::Destructive);
    }

    #[test]
    fn check_results_are_sorted_by_migration_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_migration(tmp.path(), "20260103000000_third", "SELECT 1;");
        write_migration(tmp.path(), "20260101000000_first", "SELECT 1;");
        write_migration(tmp.path(), "20260102000000_second", "SELECT 1;");

        let results = check_migrations_in_dir(tmp.path()).unwrap();
        let names: Vec<_> = results.iter().map(|(n, _)| n.as_str()).collect();
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
        assert_eq!(results[0].0, "20260101000000_valid");
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
        assert!(results[0].1.is_empty(), "first migration should be safe");
        assert!(
            !results[1].1.is_empty(),
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
        let (_, findings) = &results[0];
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, safety::RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn default_migrations_dir_is_migrations() {
        assert_eq!(DEFAULT_MIGRATIONS_DIR, "migrations");
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
        let (_, findings) = &results[0];
        assert!(
            findings
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
        let (_, findings) = &results[0];
        assert!(
            !findings
                .iter()
                .any(|f| f.operation.contains("CONCURRENTLY")),
            "opted-out CONCURRENTLY should not produce a transaction opt-out finding"
        );
    }
}
