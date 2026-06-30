//! `autumn token` -- issue and revoke API bearer tokens.
//!
//! Tokens are generated with 256 bits of OS-backed randomness, hashed with
//! SHA-256 before storage, and inserted / revoked via `psql`. The database URL
//! is resolved from `autumn.toml` or environment variables using the same
//! logic as `autumn migrate`.

use hex::encode as hex_encode;
use sha2::{Digest as _, Sha256};
use std::path::Path;
use std::process::{Command, Stdio};

/// Issue a new API token for `principal_id` and print the raw token to stdout.
///
/// `name` is a human-readable label, `scopes` are flat permission strings
/// (e.g. `posts:read`) stored as a JSON array, and `expires_at` is an optional
/// SQL/ISO-8601 timestamp after which the token is rejected.
pub fn run_issue(principal_id: &str, name: &str, scopes: &[String], expires_at: Option<&str>) {
    let database_url = resolve_database_url();
    check_psql();

    let raw_token = generate_token();
    let token_hash = sha256_hex(&raw_token);
    let scopes_json = scopes_to_json(scopes);
    let expires_at = expires_at.unwrap_or("");

    // Use psql variable substitution (:'var') to avoid any SQL injection risk.
    // psql quotes each value as a SQL string literal, so no manual escaping is
    // needed. `:'scopes'::jsonb` casts the JSON array text into the JSONB
    // column; an empty `:'expires_at'` becomes NULL via NULLIF.
    run_psql_with_vars_or_die(
        &database_url,
        "INSERT INTO api_tokens (token_hash, principal_id, name, scopes, expires_at) \
         VALUES (:'hash', :'principal', :'name', :'scopes'::jsonb, \
         NULLIF(:'expires_at', '')::timestamptz AT TIME ZONE 'UTC');",
        &[
            ("hash", &token_hash),
            ("principal", principal_id),
            ("name", name),
            ("scopes", &scopes_json),
            ("expires_at", expires_at),
        ],
    );

    // Print raw token last so it's easy to capture with $(...)
    println!("{raw_token}");
    eprintln!("\u{2713} Token issued for principal: {principal_id}");
}

/// List non-secret metadata for a principal's tokens.
pub fn run_list(principal_id: &str) {
    let database_url = resolve_database_url();
    check_psql();

    run_psql_with_vars_or_die(
        &database_url,
        "SELECT id, name, principal_id, scopes, created_at, expires_at, last_used_at, revoked_at \
         FROM api_tokens WHERE principal_id = :'principal' ORDER BY id;",
        &[("principal", principal_id)],
    );
}

/// Rotate a token: revoke `raw_token` and issue a replacement carrying the same
/// name, scopes, and expiry. Prints the new raw token to stdout.
pub fn run_rotate(raw_token: &str) {
    let database_url = resolve_database_url();
    check_psql();

    let old_hash = sha256_hex(raw_token);
    let new_token = generate_token();
    let new_hash = sha256_hex(&new_token);

    // Revoke the old row and copy its name/scopes/expiry onto a new row in a
    // single statement so the rotation is atomic.
    // The SELECT 1/COUNT(*) forces psql to error if the old token was not found
    // (already revoked or never existed), preventing a silent no-op rotation.
    run_psql_silent(
        &database_url,
        "WITH rotated AS ( \
            UPDATE api_tokens SET revoked_at = NOW() AT TIME ZONE 'utc' \
            WHERE token_hash = :'oldhash' AND revoked_at IS NULL \
                AND (expires_at IS NULL OR expires_at > NOW() AT TIME ZONE 'utc') \
            RETURNING principal_id, name, scopes, expires_at \
         ), \
         inserted AS ( \
            INSERT INTO api_tokens (token_hash, principal_id, name, scopes, expires_at) \
            SELECT :'newhash', principal_id, name, scopes, expires_at FROM rotated \
            RETURNING 1 \
         ) \
         SELECT 1 / COUNT(*) FROM inserted;",
        &[("oldhash", &old_hash), ("newhash", &new_hash)],
    );

    println!("{new_token}");
    eprintln!("\u{2713} Token rotated.");
}

/// Revoke the token identified by `raw_token`.
pub fn run_revoke(raw_token: &str) {
    let database_url = resolve_database_url();
    check_psql();

    let token_hash = sha256_hex(raw_token);

    run_psql_with_vars_or_die(
        &database_url,
        "UPDATE api_tokens SET revoked_at = NOW() AT TIME ZONE 'utc' WHERE token_hash = :'hash' AND revoked_at IS NULL;",
        &[("hash", &token_hash)],
    );
    eprintln!("\u{2713} Token revoked.");
}

// ── internals ─────────────────────────────────────────────────────────────────────────────

fn sha256_hex(input: &str) -> String {
    hex_encode(Sha256::digest(input.as_bytes()))
}

/// Render flat scope strings as a JSON array for the `scopes` JSONB column.
fn scopes_to_json(scopes: &[String]) -> String {
    let items: Vec<serde_json::Value> = scopes
        .iter()
        .map(|s| serde_json::Value::String(s.clone()))
        .collect();
    serde_json::Value::Array(items).to_string()
}

/// Produce 32 random bytes from the OS and return them as a 64-char hex string.
fn generate_token() -> String {
    hex_encode(os_random_bytes())
}

/// Read 32 cryptographically-random bytes from the OS.
fn os_random_bytes() -> [u8; 32] {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS random source unavailable");
    buf
}

/// Resolve the database URL from autumn.toml and environment variables.
///
/// Precedence (highest to lowest):
/// 1. `AUTUMN_DATABASE__PRIMARY_URL` environment variable
/// 2. `AUTUMN_DATABASE__URL` environment variable
/// 3. `DATABASE_URL` environment variable
/// 4. `database.primary_url` from `autumn.toml`
/// 5. `database.url` from `autumn.toml`
fn resolve_database_url() -> String {
    let config_table = read_autumn_toml_table();
    if let Some(url) =
        resolve_primary_database_url_from_sources(|key| std::env::var(key), config_table.as_ref())
    {
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

/// Run a SQL statement using psql variable substitution.
///
/// Each `(name, value)` in `vars` is passed as `-v name=value`, and the SQL
/// may reference them as `:'name'` (psql quotes the value as a SQL string
/// literal, preventing SQL injection).
fn run_psql_with_vars_or_die(database_url: &str, sql: &str, vars: &[(&str, &str)]) {
    run_psql_impl(database_url, sql, vars, false);
}

/// Like [`run_psql_with_vars_or_die`] but discards psql stdout so that
/// validation-only SELECT results don't pollute the caller's stdout capture
/// (e.g. `TOKEN=$(autumn token rotate …)`).
fn run_psql_silent(database_url: &str, sql: &str, vars: &[(&str, &str)]) {
    run_psql_impl(database_url, sql, vars, true);
}

fn run_psql_impl(database_url: &str, sql: &str, vars: &[(&str, &str)], suppress_stdout: bool) {
    let mut cmd = Command::new("psql");
    cmd.arg(database_url);
    for (name, value) in vars {
        cmd.args(["-v", &format!("{name}={value}")]);
    }
    cmd.args(["-c", sql]);
    if suppress_stdout {
        cmd.stdout(Stdio::null());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_64_chars() {
        let h = sha256_hex("test_token");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sha256_hex_is_deterministic() {
        assert_eq!(sha256_hex("abc"), sha256_hex("abc"));
    }

    #[test]
    fn sha256_hex_differs_on_different_input() {
        assert_ne!(sha256_hex("abc"), sha256_hex("xyz"));
    }

    #[test]
    fn scopes_to_json_renders_array() {
        assert_eq!(scopes_to_json(&[]), "[]");
        assert_eq!(
            scopes_to_json(&["posts:read".to_owned(), "posts:write".to_owned()]),
            r#"["posts:read","posts:write"]"#
        );
    }

    #[test]
    fn generate_token_produces_64_hex_chars() {
        let t = generate_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_token_is_not_all_zeros() {
        let t = generate_token();
        assert_ne!(t, "0".repeat(64));
    }

    #[test]
    fn resolve_prefers_autumn_database_url_over_database_url() {
        let url = temp_env::with_vars(
            [
                ("AUTUMN_DATABASE__PRIMARY_URL", None::<&str>),
                ("AUTUMN_DATABASE__URL", Some("postgres://autumn-primary")),
                ("DATABASE_URL", Some("postgres://fallback")),
            ],
            resolve_database_url,
        );
        assert_eq!(url, "postgres://autumn-primary");
    }

    #[test]
    fn resolve_falls_back_to_database_url_when_autumn_unset() {
        let url = temp_env::with_vars(
            [
                ("AUTUMN_DATABASE__PRIMARY_URL", None::<&str>),
                ("AUTUMN_DATABASE__URL", None::<&str>),
                ("DATABASE_URL", Some("postgres://fallback")),
            ],
            resolve_database_url,
        );
        assert_eq!(url, "postgres://fallback");
    }

    #[test]
    fn resolve_prefers_primary_database_url_env() {
        let env = |key: &str| match key {
            "AUTUMN_DATABASE__PRIMARY_URL" => Ok("postgres://primary-env".to_owned()),
            "AUTUMN_DATABASE__URL" => Ok("postgres://legacy-env".to_owned()),
            "DATABASE_URL" => Ok("postgres://fallback-env".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let url = resolve_primary_database_url_from_sources(env, None).unwrap();
        assert_eq!(url, "postgres://primary-env");
    }

    #[test]
    fn resolve_reads_primary_database_url_from_toml() {
        let table = toml::from_str::<toml::Table>(
            r#"
            [database]
            primary_url = "postgres://primary-toml"
            url = "postgres://legacy-toml"
            "#,
        )
        .unwrap();
        let env =
            |_: &str| -> Result<String, std::env::VarError> { Err(std::env::VarError::NotPresent) };

        let url = resolve_primary_database_url_from_sources(env, Some(&table)).unwrap();

        assert_eq!(url, "postgres://primary-toml");
    }

    #[test]
    fn check_psql_does_not_panic_when_available() {
        // Run only when psql is on PATH; skip otherwise to avoid process::exit.
        if std::process::Command::new("psql")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            check_psql();
        }
    }
}
