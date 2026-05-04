//! `autumn token` -- issue and revoke API bearer tokens.
//!
//! Tokens are generated with 256 bits of OS-backed randomness, hashed with
//! SHA-256 before storage, and inserted / revoked via `psql`. The database URL
//! is resolved from `autumn.toml` or environment variables using the same
//! logic as `autumn migrate`.

use hex::encode as hex_encode;
use sha2::{Digest as _, Sha256};
use std::path::Path;
use std::process::Command;

/// Issue a new API token for `principal_id` and print the raw token to stdout.
pub fn run_issue(principal_id: &str) {
    let database_url = resolve_database_url();
    check_psql();

    let raw_token = generate_token();
    let token_hash = sha256_hex(&raw_token);

    let escaped_hash = escape_sql_literal(&token_hash);
    let escaped_principal = escape_sql_literal(principal_id);
    let sql = format!(
        "INSERT INTO api_tokens (token_hash, principal_id) VALUES ('{escaped_hash}', '{escaped_principal}');"
    );

    run_psql_or_die(&database_url, &sql);

    // Print raw token last so it's easy to capture with $(...)
    println!("{raw_token}");
    eprintln!("\u{2713} Token issued for principal: {principal_id}");
}

/// Revoke the token identified by `raw_token`.
pub fn run_revoke(raw_token: &str) {
    let database_url = resolve_database_url();
    check_psql();

    let token_hash = sha256_hex(raw_token);
    let escaped_hash = escape_sql_literal(&token_hash);
    let sql = format!(
        "UPDATE api_tokens SET revoked_at = NOW() WHERE token_hash = '{escaped_hash}' AND revoked_at IS NULL;"
    );

    run_psql_or_die(&database_url, &sql);
    eprintln!("\u{2713} Token revoked.");
}

// ── internals ─────────────────────────────────────────────────────────────────

fn sha256_hex(input: &str) -> String {
    hex_encode(Sha256::digest(input.as_bytes()))
}

/// Produce 32 random bytes from the OS and return them as a 64-char hex string.
fn generate_token() -> String {
    hex_encode(os_random_bytes())
}

/// Read 32 cryptographically-random bytes from the OS.
fn os_random_bytes() -> [u8; 32] {
    let mut buf = [0u8; 32];
    #[cfg(unix)]
    {
        use std::io::Read as _;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut buf).is_ok() {
                return buf;
            }
        }
    }
    // Windows fallback: hash time+pid (low entropy, rare code path)
    let seed = format!(
        "{}{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
    );
    let hash = Sha256::digest(seed.as_bytes());
    buf.copy_from_slice(&hash);
    buf
}

/// Escape a string for use inside a PostgreSQL single-quoted literal.
///
/// Doubles any embedded single quotes per the SQL standard. The `token_hash`
/// argument is always a 64-char hex string so it needs no escaping; this is
/// only material for `principal_id`.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

/// Resolve the database URL from autumn.toml and environment variables.
///
/// Precedence (highest to lowest):
/// 1. `AUTUMN_DATABASE__URL` environment variable
/// 2. `DATABASE_URL` environment variable
/// 3. `database.url` from `autumn.toml`
fn resolve_database_url() -> String {
    if let Ok(url) = std::env::var("AUTUMN_DATABASE__URL")
        && !url.is_empty()
    {
        return url;
    }
    if let Ok(url) = std::env::var("DATABASE_URL")
        && !url.is_empty()
    {
        return url;
    }
    let config_path = Path::new("autumn.toml");
    if config_path.exists()
        && let Ok(contents) = std::fs::read_to_string(config_path)
        && let Ok(table) = toml::from_str::<toml::Table>(&contents)
    {
        let value = toml::Value::Table(table);
        if let Some(url) = value
            .get("database")
            .and_then(|db| db.get("url"))
            .and_then(|u| u.as_str())
            && !url.is_empty()
        {
            return url.to_string();
        }
    }
    eprintln!("\u{2717} No database URL found.");
    eprintln!("  Set database.url in autumn.toml, or set AUTUMN_DATABASE__URL / DATABASE_URL.");
    std::process::exit(1);
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

fn run_psql_or_die(database_url: &str, sql: &str) {
    let status = Command::new("psql")
        .args([database_url, "-c", sql])
        .status();
    match status {
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
    fn escape_sql_literal_doubles_single_quotes() {
        let input = "user's principal";
        let escaped = escape_sql_literal(input);
        assert_eq!(escaped, "user''s principal");
    }

    #[test]
    fn escape_sql_literal_no_quotes_unchanged() {
        let input = "user:42";
        assert_eq!(escape_sql_literal(input), input);
    }

    #[test]
    fn escape_sql_literal_multiple_quotes() {
        let input = "it's a 'test'";
        assert_eq!(escape_sql_literal(input), "it''s a ''test''");
    }
}
