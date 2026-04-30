//! `autumn seed` -- run the project's seed binary to populate the database.
//!
//! Delegates to `cargo run --bin seed` after:
//!   1. Verifying `src/bin/seed.rs` exists.
//!   2. Checking for pending migrations via the diesel CLI.
//!
//! The seed binary receives the active profile through the `AUTUMN_ENV`
//! environment variable, matching how the rest of the framework resolves
//! configuration profiles.

use std::path::Path;
use std::process::Command;

/// Errors surfaced by the seed runner.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SeedError {
    #[error(
        "no seed binary found; create `src/bin/seed.rs` or run `autumn generate seed`\n\
         See: https://autumn.rs/guide/seeding"
    )]
    MissingSeedBinary,

    #[error(
        "pending migrations detected; run `autumn migrate` before `autumn seed`"
    )]
    PendingMigrations,
}

/// Returns `true` if the seed binary source file exists at `path`.
pub(crate) fn seed_binary_exists_at(path: &Path) -> bool {
    path.is_file()
}

/// Resolve the database URL for the pending-migration check.
///
/// Precedence (highest to lowest):
/// 1. `AUTUMN_DATABASE__URL` env var
/// 2. `DATABASE_URL` env var
/// 3. `database.url` in `autumn.toml`
pub(crate) fn resolve_database_url_with_env<F>(env_var: F) -> Result<String, SeedError>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    if let Ok(url) = env_var("AUTUMN_DATABASE__URL")
        && !url.is_empty()
    {
        return Ok(url);
    }
    if let Ok(url) = env_var("DATABASE_URL")
        && !url.is_empty()
    {
        return Ok(url);
    }

    let config_path = Path::new("autumn.toml");
    if config_path.exists()
        && let Ok(contents) = std::fs::read_to_string(config_path)
        && let Ok(table) = toml::from_str::<toml::Table>(&contents)
    {
        let value = toml::Value::Table(table);
        if let Some(url) = value
            .get("database")
            .and_then(|db: &toml::Value| db.get("url"))
            .and_then(|u: &toml::Value| u.as_str())
            && !url.is_empty()
        {
            return Ok(url.to_string());
        }
    }

    Err(SeedError::MissingSeedBinary)
}

/// Check whether there are pending (unapplied) migrations.
///
/// Runs `diesel migration list` and scans for `[ ]` markers that indicate
/// unapplied migrations. Returns `Ok(())` if all migrations are applied or
/// if diesel CLI is unavailable (in which case a warning is printed).
fn check_pending_migrations(database_url: &str, migrations_dir: &str) -> Result<(), SeedError> {
    let output = Command::new("diesel")
        .args(["migration", "list", "--migration-dir", migrations_dir])
        .env("DATABASE_URL", database_url)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");
            if combined.lines().any(|line| line.contains("[ ]")) {
                return Err(SeedError::PendingMigrations);
            }
            Ok(())
        }
        Err(_) => {
            eprintln!(
                "  \u{26a0} diesel CLI not found; skipping pending-migration check.\n  \
                 Install with: cargo install diesel_cli --no-default-features --features postgres"
            );
            Ok(())
        }
    }
}

/// Entry point for `autumn seed`.
pub fn run(profile: &str, package: Option<&str>) {
    eprintln!("\u{1F342} autumn seed\n");
    eprintln!("  Profile: {profile}");

    let seed_path = Path::new("src/bin/seed.rs");
    if !seed_binary_exists_at(seed_path) {
        eprintln!("\u{2717} {}", SeedError::MissingSeedBinary);
        std::process::exit(1);
    }

    // Best-effort pending migration check when diesel CLI is available.
    let db_url = std::env::var("AUTUMN_DATABASE__URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_default();
    if !db_url.is_empty() {
        let migrations_dir = if Path::new("migrations").is_dir() {
            "migrations"
        } else {
            ""
        };
        if !migrations_dir.is_empty() {
            if let Err(e) = check_pending_migrations(&db_url, migrations_dir) {
                eprintln!("\u{2717} {e}");
                std::process::exit(1);
            }
        }
    }

    eprintln!("  Running seed binary...\n");

    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--bin", "seed"]);
    if let Some(pkg) = package {
        cmd.args(["--package", pkg]);
    }
    cmd.env("AUTUMN_ENV", profile);
    cmd.env("AUTUMN_PROFILE", profile);

    let status = cmd.status();
    match status {
        Ok(s) if s.success() => {
            eprintln!("\n\u{2713} Seed completed successfully.");
        }
        Ok(_) => {
            eprintln!("\n\u{2717} Seed binary exited with a non-zero status.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\u{2717} Failed to run cargo: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    // ── SeedError messages ─────────────────────────────────────────────────

    #[test]
    fn missing_seed_binary_error_mentions_src_bin_seed() {
        let msg = SeedError::MissingSeedBinary.to_string();
        assert!(
            msg.contains("src/bin/seed.rs"),
            "error should mention src/bin/seed.rs, got: {msg}"
        );
    }

    #[test]
    fn missing_seed_binary_error_mentions_generate_seed() {
        let msg = SeedError::MissingSeedBinary.to_string();
        assert!(
            msg.contains("generate seed"),
            "error should mention `autumn generate seed`, got: {msg}"
        );
    }

    #[test]
    fn pending_migrations_error_mentions_autumn_migrate() {
        let msg = SeedError::PendingMigrations.to_string();
        assert!(
            msg.contains("autumn migrate"),
            "error should mention `autumn migrate`, got: {msg}"
        );
    }

    #[test]
    fn pending_migrations_error_mentions_pending() {
        let msg = SeedError::PendingMigrations.to_string();
        assert!(
            msg.contains("pending"),
            "error should mention pending migrations, got: {msg}"
        );
    }

    // ── seed_binary_exists_at ──────────────────────────────────────────────

    #[test]
    fn seed_binary_exists_at_returns_false_for_missing_path() {
        assert!(!seed_binary_exists_at(Path::new(
            "/nonexistent/src/bin/seed.rs"
        )));
    }

    #[test]
    fn seed_binary_exists_at_returns_true_when_file_present() {
        let tmp = TempDir::new().unwrap();
        let seed_path = tmp.path().join("src/bin/seed.rs");
        std::fs::create_dir_all(seed_path.parent().unwrap()).unwrap();
        std::fs::write(&seed_path, "fn main() {}").unwrap();
        assert!(seed_binary_exists_at(&seed_path));
    }

    #[test]
    fn seed_binary_exists_at_returns_false_for_directory() {
        let tmp = TempDir::new().unwrap();
        let seed_dir = tmp.path().join("src/bin/seed.rs");
        std::fs::create_dir_all(&seed_dir).unwrap();
        // seed_dir is a directory, not a file
        assert!(!seed_binary_exists_at(&seed_dir));
    }

    // ── resolve_database_url_with_env ──────────────────────────────────────

    #[test]
    fn resolve_db_url_prefers_autumn_database_url() {
        let env = |key: &str| match key {
            "AUTUMN_DATABASE__URL" => Ok("postgres://autumn:5432/db".to_string()),
            "DATABASE_URL" => Ok("postgres://plain:5432/db".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let url = resolve_database_url_with_env(env).unwrap();
        assert_eq!(url, "postgres://autumn:5432/db");
    }

    #[test]
    fn resolve_db_url_falls_back_to_database_url() {
        let env = |key: &str| match key {
            "DATABASE_URL" => Ok("postgres://fallback:5432/db".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let url = resolve_database_url_with_env(env).unwrap();
        assert_eq!(url, "postgres://fallback:5432/db");
    }

    #[test]
    fn resolve_db_url_returns_err_when_no_env_no_toml() {
        let env =
            |_: &str| -> Result<String, std::env::VarError> { Err(std::env::VarError::NotPresent) };
        // No autumn.toml in the test working directory path being injected
        assert!(resolve_database_url_with_env(env).is_err());
    }

    #[test]
    fn resolve_db_url_ignores_empty_env_var() {
        let env = |key: &str| match key {
            "AUTUMN_DATABASE__URL" => Ok(String::new()),
            "DATABASE_URL" => Ok("postgres://real:5432/db".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let url = resolve_database_url_with_env(env).unwrap();
        assert_eq!(url, "postgres://real:5432/db");
    }
}
