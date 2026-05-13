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

use std::path::Path;
use std::process::Command;

use autumn_web::auth::API_TOKEN_MIGRATIONS;
use autumn_web::migrate::{EmbeddedMigrations, MigrationError, MigrationResult};

/// Default directory containing Diesel migration files.
const DEFAULT_MIGRATIONS_DIR: &str = "migrations";

/// Subcommands for `autumn migrate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateAction {
    /// Run all pending migrations.
    Run,
    /// Show migration status (pending / applied).
    Status,
}

/// Run the migrate command.
pub fn run(action: MigrateAction) {
    eprintln!("\u{1F342} autumn migrate\n");

    // 1. Resolve database URL from autumn.toml + env
    let database_url = resolve_database_url();

    // 2. Resolve migrations directory
    let migrations_dir = resolve_migrations_dir();

    // 3. Check that diesel CLI is available
    check_diesel_cli();

    // 4. Execute the appropriate diesel command
    match action {
        MigrateAction::Run => {
            run_migrations(&database_url, &migrations_dir);
            run_framework_migrations(&database_url);
        }
        MigrateAction::Status => {
            show_status(&database_url, &migrations_dir);
            show_framework_status(&database_url);
        }
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

/// Run pending migrations via `diesel migration run`.
fn run_migrations(database_url: &str, migrations_dir: &str) {
    eprintln!("  Running pending migrations...\n");
    run_diesel_migration_run(database_url, Path::new(migrations_dir), "Migrations");
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
    run_pending(database_url, API_TOKEN_MIGRATIONS)
}

fn run_diesel_migration_run(database_url: &str, migrations_dir: &Path, label: &str) {
    let status = Command::new("diesel")
        .args(["migration", "run", "--migration-dir"])
        .arg(migrations_dir)
        .env("DATABASE_URL", database_url)
        .status();

    match status {
        Ok(s) if s.success() => {
            eprintln!("\n\u{2713} {label} applied successfully.");
        }
        Ok(_) => {
            eprintln!(
                "\n\u{2717} Migration failed in {}. Check the error output above for the failing SQL.",
                migrations_dir.display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "\u{2717} Failed to run diesel migration run for {}: {e}",
                migrations_dir.display()
            );
            std::process::exit(1);
        }
    }
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
    pending_migrations(database_url, API_TOKEN_MIGRATIONS)
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

    #[test]
    fn migrate_action_eq() {
        assert_eq!(MigrateAction::Run, MigrateAction::Run);
        assert_eq!(MigrateAction::Status, MigrateAction::Status);
        assert_ne!(MigrateAction::Run, MigrateAction::Status);
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
    fn embedded_api_token_migrations_include_real_schema_migration() {
        use autumn_web::reexports::diesel::migration::{Migration, MigrationSource};
        use autumn_web::reexports::diesel::pg::Pg;

        let migrations: Vec<Box<dyn Migration<Pg>>> =
            autumn_web::auth::API_TOKEN_MIGRATIONS.migrations().unwrap();
        let names: Vec<_> = migrations
            .iter()
            .map(|migration| migration.name().to_string())
            .collect();

        assert!(
            names
                .iter()
                .any(|name| name == "20260512000000_create_api_tokens"),
            "API token embedded migrations must include the timestamped schema migration: {names:?}"
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
}
