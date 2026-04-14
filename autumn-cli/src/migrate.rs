//! `autumn migrate` -- run or inspect Diesel database migrations.
//!
//! Because the CLI cannot embed the user's application-specific migrations
//! (they live in the user's crate via `embed_migrations!`), this module
//! delegates to the `diesel` CLI tool. It reads the database URL from
//! `autumn.toml` + environment variables, then shells out to
//! `diesel migration run` or `diesel migration pending`.
//!
//! The user's application binary is the canonical way to run embedded
//! migrations (via `.migrations()` on `AppBuilder`). This CLI command
//! is a convenience wrapper for explicit migration management.

use std::path::Path;
use std::process::Command;

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
        MigrateAction::Run => run_migrations(&database_url, &migrations_dir),
        MigrateAction::Status => show_status(&database_url, &migrations_dir),
    }
}

/// Resolve the database URL from autumn.toml and environment variables.
///
/// Precedence (highest to lowest):
/// 1. `AUTUMN_DATABASE__URL` environment variable
/// 2. `DATABASE_URL` environment variable
/// 3. `database.url` from `autumn.toml`
fn resolve_database_url() -> String {
    resolve_database_url_with_env(|key| std::env::var(key))
}

fn resolve_database_url_with_env<F>(env_var: F) -> String
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    // Check env overrides first
    if let Ok(url) = env_var("AUTUMN_DATABASE__URL") {
        if !url.is_empty() {
            return url;
        }
    }
    if let Ok(url) = env_var("DATABASE_URL") {
        if !url.is_empty() {
            return url;
        }
    }

    // Try loading from autumn.toml
    let config_path = Path::new("autumn.toml");
    if config_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(config_path) {
            if let Ok(table) = toml::from_str::<toml::Table>(&contents) {
                let value = toml::Value::Table(table);
                if let Some(url) = value
                    .get("database")
                    .and_then(|db: &toml::Value| db.get("url"))
                    .and_then(|u: &toml::Value| u.as_str())
                {
                    if !url.is_empty() {
                        return url.to_string();
                    }
                }
            }
        }
    }

    eprintln!("\u{2717} No database URL found.");
    eprintln!("  Set database.url in autumn.toml, or set AUTUMN_DATABASE__URL / DATABASE_URL.");
    std::process::exit(1);
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

    let status = Command::new("diesel")
        .args(["migration", "run", "--migration-dir", migrations_dir])
        .env("DATABASE_URL", database_url)
        .status();

    match status {
        Ok(s) if s.success() => {
            eprintln!("\n\u{2713} Migrations applied successfully.");
        }
        Ok(_) => {
            eprintln!(
                "\n\u{2717} Migration failed. Check the error output above for the failing SQL."
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\u{2717} Failed to run diesel migration run: {e}");
            std::process::exit(1);
        }
    }
}

/// Show migration status via `diesel migration pending`.
fn show_status(database_url: &str, migrations_dir: &str) {
    eprintln!("  Checking migration status...\n");

    // `diesel migration list` shows all migrations and their status
    let status = Command::new("diesel")
        .args(["migration", "list", "--migration-dir", migrations_dir])
        .env("DATABASE_URL", database_url)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            eprintln!("\n\u{2717} Failed to check migration status.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\u{2717} Failed to run diesel migration list: {e}");
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
}
