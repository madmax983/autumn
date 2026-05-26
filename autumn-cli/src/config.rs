//! `autumn config` — inspect and mutate runtime configuration values.
//!
//! All commands connect directly to the configured Postgres database via
//! `psql`, following the same URL-resolution strategy as `autumn token`.
//!
//! # Commands
//!
//! ```text
//! autumn config list                    # list all overrides (key, value, updated_at)
//! autumn config get <key>               # print the current stored value for a key
//! autumn config set <key> <value>       # set (or replace) a key
//! autumn config unset <key>             # remove the override, restoring the default
//! autumn config history <key>           # show the change history for a key
//! ```

use std::path::Path;
use std::process::Command;

/// Options for `autumn config list`.
pub struct ListOptions;

/// Options for `autumn config get <key>`.
pub struct GetOptions {
    pub key: String,
}

/// Options for `autumn config set <key> <value>`.
pub struct SetOptions {
    pub key: String,
    pub value: String,
    pub actor: Option<String>,
}

/// Options for `autumn config unset <key>`.
pub struct UnsetOptions {
    pub key: String,
    pub actor: Option<String>,
}

/// Options for `autumn config history <key>`.
pub struct HistoryOptions {
    pub key: String,
    pub limit: usize,
}

// ── Public entry points ────────────────────────────────────────────────────────

/// Run `autumn config list`.
pub fn run_list(_opts: &ListOptions) {
    let url = resolve_database_url();
    check_psql();
    run_psql_or_die(
        &url,
        "SELECT key, raw_value, updated_at \
         FROM autumn_runtime_config_values \
         ORDER BY key;",
    );
}

/// Run `autumn config get <key>`.
pub fn run_get(opts: &GetOptions) {
    let url = resolve_database_url();
    check_psql();
    // Probe for existence with tuples-only output so 0 rows vs 1 row is unambiguous
    // regardless of locale (no "(0 rows)" footer to parse).
    let mut probe = Command::new("psql");
    probe.arg(&url).args(["-t", "-A"]);
    probe.args(["-v", &format!("key={}", opts.key)]).args([
        "-c",
        "SELECT 1 FROM autumn_runtime_config_values WHERE key = :'key';",
    ]);
    match probe.output() {
        Ok(out) if out.status.success() => {
            if String::from_utf8_lossy(&out.stdout).trim().is_empty() {
                eprintln!("\u{2717} Key '{}' has no active override.", opts.key);
                std::process::exit(1);
            }
        }
        Ok(_) => {
            eprintln!("\u{2717} psql probe failed. Check the output above.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\u{2717} Failed to run psql: {e}");
            std::process::exit(1);
        }
    }
    // Key exists — print the full row with headers.
    run_psql_with_vars_or_die(
        &url,
        "SELECT key, raw_value, updated_at \
         FROM autumn_runtime_config_values \
         WHERE key = :'key';",
        &[("key", &opts.key)],
    );
}

/// Run `autumn config set <key> <value>`.
pub fn run_set(opts: &SetOptions) {
    let url = resolve_database_url();
    check_psql();
    let actor = opts.actor.as_deref().unwrap_or("cli");

    // Acquire a per-key advisory lock before reading the prior value so that
    // concurrent writers on a brand-new key are serialised: T2 blocks here
    // until T1 commits, and the next statement then sees T1's committed row
    // under READ COMMITTED's per-statement snapshot.  The CTE's FOR UPDATE
    // covers existing-key races; the advisory lock covers new-key races.
    let sql = "BEGIN; \
        SELECT pg_advisory_xact_lock(1, hashtext(:'key')); \
        WITH \
            prior AS ( \
                SELECT raw_value \
                FROM autumn_runtime_config_values \
                WHERE key = :'key' \
            ), \
            upsert AS ( \
                INSERT INTO autumn_runtime_config_values (key, raw_value, updated_at) \
                    VALUES (:'key', :'value', NOW()) \
                    ON CONFLICT (key) DO UPDATE \
                        SET raw_value = EXCLUDED.raw_value, \
                            updated_at = EXCLUDED.updated_at \
            ) \
        INSERT INTO autumn_runtime_config_changes (key, old_value, new_value, actor) \
            VALUES ( \
                :'key', \
                (SELECT raw_value FROM prior), \
                :'value', \
                :'actor' \
            ); \
        COMMIT;";

    run_psql_with_vars_or_die(
        &url,
        sql,
        &[("key", &opts.key), ("value", &opts.value), ("actor", actor)],
    );

    eprintln!(
        "\u{2713} Set '{key}' = '{value}'",
        key = opts.key,
        value = opts.value
    );
}

/// Run `autumn config unset <key>`.
pub fn run_unset(opts: &UnsetOptions) {
    let url = resolve_database_url();
    check_psql();
    let actor = opts.actor.as_deref().unwrap_or("cli");

    // DELETE RETURNING captures the value at the exact moment the row is removed,
    // so no concurrent update can produce a stale old_value in the audit row.
    let sql = "BEGIN; \
        WITH \
            removed AS ( \
                DELETE FROM autumn_runtime_config_values \
                WHERE key = :'key' \
                RETURNING raw_value \
            ) \
        INSERT INTO autumn_runtime_config_changes (key, old_value, new_value, actor) \
            SELECT :'key', raw_value, NULL, :'actor' \
            FROM removed; \
        COMMIT;";

    run_psql_with_vars_or_die(&url, sql, &[("key", &opts.key), ("actor", actor)]);
    eprintln!(
        "\u{2713} Unset '{key}' (reverted to compile-time default)",
        key = opts.key
    );
}

/// Run `autumn config history <key>`.
pub fn run_history(opts: &HistoryOptions) {
    let url = resolve_database_url();
    check_psql();
    let limit = opts.limit.to_string();
    run_psql_with_vars_or_die(
        &url,
        "SELECT id, key, old_value, new_value, actor, changed_at \
         FROM autumn_runtime_config_changes \
         WHERE key = :'key' \
         ORDER BY changed_at DESC \
         LIMIT :'limit'::int;",
        &[("key", &opts.key), ("limit", &limit)],
    );
}

// ── Database URL resolution (mirrors token.rs) ────────────────────────────────

pub fn resolve_database_url() -> String {
    let config_table = read_autumn_toml_table();
    if let Some(url) =
        resolve_primary_database_url_from_sources(|key| std::env::var(key), config_table.as_ref())
    {
        return url;
    }

    eprintln!("\u{2717} No database URL found.");
    eprintln!(
        "  Set database.primary_url (or database.url) in autumn.toml, \
         or set AUTUMN_DATABASE__PRIMARY_URL / AUTUMN_DATABASE__URL / DATABASE_URL."
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

pub fn resolve_primary_database_url_from_sources<F>(
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

// ── psql helpers ──────────────────────────────────────────────────────────────

fn check_psql() {
    match Command::new("psql").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let v = String::from_utf8_lossy(&out.stdout);
            eprintln!("  Using {}", v.trim());
        }
        _ => {
            eprintln!("\u{2717} psql not found on PATH.");
            eprintln!("  Install PostgreSQL client tools (e.g. `apt install postgresql-client`).");
            std::process::exit(1);
        }
    }
}

fn run_psql_or_die(database_url: &str, sql: &str) {
    let status = Command::new("psql")
        .args([database_url, "-v", "ON_ERROR_STOP=on", "-c", sql])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            eprintln!("\u{2717} psql command failed.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\u{2717} Failed to run psql: {e}");
            std::process::exit(1);
        }
    }
}

fn run_psql_with_vars_or_die(database_url: &str, sql: &str, vars: &[(&str, &str)]) {
    let mut cmd = Command::new("psql");
    cmd.arg(database_url);
    cmd.args(["-v", "ON_ERROR_STOP=on"]);
    for (name, value) in vars {
        cmd.args(["-v", &format!("{name}={value}")]);
    }
    cmd.args(["-c", sql]);
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(_) => {
            eprintln!("\u{2717} psql command failed. Check the output above.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\u{2717} Failed to run psql: {e}");
            std::process::exit(1);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_primary_url_env_var() {
        let env = |key: &str| match key {
            "AUTUMN_DATABASE__PRIMARY_URL" => Ok("postgres://primary".to_owned()),
            "AUTUMN_DATABASE__URL" => Ok("postgres://legacy".to_owned()),
            "DATABASE_URL" => Ok("postgres://fallback".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let url = resolve_primary_database_url_from_sources(env, None).unwrap();
        assert_eq!(url, "postgres://primary");
    }

    #[test]
    fn resolve_falls_back_to_legacy_env_var() {
        let env = |key: &str| match key {
            "AUTUMN_DATABASE__URL" => Ok("postgres://legacy".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let url = resolve_primary_database_url_from_sources(env, None).unwrap();
        assert_eq!(url, "postgres://legacy");
    }

    #[test]
    fn resolve_falls_back_to_database_url_env_var() {
        let env = |key: &str| match key {
            "DATABASE_URL" => Ok("postgres://fallback".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let url = resolve_primary_database_url_from_sources(env, None).unwrap();
        assert_eq!(url, "postgres://fallback");
    }

    #[test]
    fn resolve_reads_primary_url_from_toml() {
        let table = toml::from_str::<toml::Table>(
            r#"
            [database]
            primary_url = "postgres://from-toml"
            "#,
        )
        .unwrap();
        let env = |_: &str| Err(std::env::VarError::NotPresent);
        let url = resolve_primary_database_url_from_sources(env, Some(&table)).unwrap();
        assert_eq!(url, "postgres://from-toml");
    }

    #[test]
    fn resolve_reads_url_from_toml_when_primary_url_absent() {
        let table = toml::from_str::<toml::Table>(
            r#"
            [database]
            url = "postgres://legacy-toml"
            "#,
        )
        .unwrap();
        let env = |_: &str| Err(std::env::VarError::NotPresent);
        let url = resolve_primary_database_url_from_sources(env, Some(&table)).unwrap();
        assert_eq!(url, "postgres://legacy-toml");
    }

    #[test]
    fn resolve_returns_none_when_no_source_found() {
        let env = |_: &str| Err(std::env::VarError::NotPresent);
        let url = resolve_primary_database_url_from_sources(env, None);
        assert!(url.is_none());
    }
}
