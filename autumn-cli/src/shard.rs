//! `autumn shard move-slot` — move a set of tenants' rows from one configured
//! shard to another (issue #1209 §3c).
//!
//! This is the framework-integrated counterpart to the worked
//! `examples/bookmarks-sharded` move tool: it resolves `--from` / `--to` by
//! their `[[database.shards]]` names (honoring `--profile` and env overrides,
//! exactly like `autumn migrate`), then for the given tenant key(s):
//!
//!   1. copies their rows source → destination,
//!   2. verifies the move (row counts **and** a `to_jsonb` content checksum
//!      match on both shards),
//!   3. deletes the source rows only with `--confirm`, after verification.
//!
//! It **never** edits the slot map — copy and verify, cut the slot over in
//! `autumn.toml`, then re-run with `--confirm` to delete. All columns
//! (including the primary key) are copied, so references that point at a moved
//! row stay valid across the move; this assumes the usual `BIGSERIAL`/explicit
//! PKs rather than `GENERATED ALWAYS AS IDENTITY`.
//!
//! Row movement uses `psql`'s `\copy` over a pipe (the same "shell out to the
//! standard Postgres tool" approach `autumn migrate` takes with `diesel`).

use std::process::{Command, Stdio};

/// Parsed arguments for `autumn shard move-slot`.
pub struct MoveSlotArgs {
    pub from: String,
    pub to: String,
    pub table: String,
    pub key_column: String,
    pub tenants: Vec<String>,
    pub confirm: bool,
    pub profile: Option<String>,
}

pub fn run_move_slot(args: &MoveSlotArgs) {
    eprintln!("\u{1F342} autumn shard move-slot\n");

    if args.tenants.is_empty() {
        fail("at least one --tenant is required");
    }
    if args.from == args.to {
        fail("--from and --to must be different shards");
    }
    validate_identifier("--table", &args.table);
    validate_identifier("--key-column", &args.key_column);

    // Resolve shard names → URLs through the same config + profile + env stack
    // as `autumn migrate` (reusing migrate's resolution helpers).
    let table = crate::migrate::read_autumn_toml_table_with_profile(args.profile.as_deref());
    let shards = crate::migrate::resolve_shard_database_urls_from_sources(
        |k| std::env::var(k),
        table.as_ref(),
    );

    let from_url = resolve_shard_url(&args.from, &shards).unwrap_or_else(|e| fail(&e));
    let to_url = resolve_shard_url(&args.to, &shards).unwrap_or_else(|e| fail(&e));

    check_psql();

    let filter = build_key_filter(&args.key_column, &args.tenants);
    eprintln!(
        "Moving {} tenant(s) on table {:?}: shard {:?} \u{2192} shard {:?}",
        args.tenants.len(),
        args.table,
        args.from,
        args.to
    );

    // ── 1. Snapshot source and destination ────────────────────────────────
    // Verify first so re-runs with `--confirm` are idempotent: if the dry-run
    // already copied the rows to the destination and the slot was then flipped,
    // re-running with `--confirm` must skip the copy (which would fail on
    // duplicate PKs) and go straight to delete.
    eprintln!("\u{2192} Checking source\u{2026}");
    let (src_count, src_sum) = snapshot(&from_url, &args.table, &filter);
    let (dst_count, dst_sum) = snapshot(&to_url, &args.table, &filter);

    let already_synced = src_count == dst_count && src_sum == dst_sum;
    if already_synced && src_count != "0" {
        eprintln!("\u{2713} Destination already matches source (previous dry-run); skipping copy.");
    } else if src_count == "0" {
        eprintln!("\u{26A0}\u{FE0F}  No rows found on source shard for the given tenant(s).");
    } else {
        // ── 2. Copy source → destination ──────────────────────────────────
        eprintln!("\u{2192} Copying rows\u{2026}");
        copy_rows(&from_url, &to_url, &args.table, &filter);

        // Re-verify after copy.
        let (dst_count2, dst_sum2) = snapshot(&to_url, &args.table, &filter);
        eprintln!("   source: count={src_count} checksum={src_sum}");
        eprintln!("   dest:   count={dst_count2} checksum={dst_sum2}");
        if src_count != dst_count2 || src_sum != dst_sum2 {
            fail("verification FAILED: destination does not match source. No rows deleted.");
        }
    }
    eprintln!("\u{2713} Verified: destination matches source.");

    // ── 3. Delete from source (only with --confirm) ───────────────────────
    if !args.confirm {
        eprintln!(
            "\u{2713} Copy verified but source rows were KEPT (no --confirm).\n  \
             Next: move these tenants' slot(s) to {:?} in autumn.toml and deploy,\n  \
             then re-run with --confirm to delete the stale source rows.",
            args.to
        );
        return;
    }
    eprintln!("\u{2192} Deleting rows from source (--confirm)\u{2026}");
    run_psql(
        &from_url,
        &[
            "--single-transaction",
            "-c",
            &delete_sql(&args.table, &filter),
        ],
    );
    eprintln!(
        "\u{2713} Done. Source rows removed; shard {:?} now owns these tenants.\n  \
         Ensure the slot map in autumn.toml routes them to {:?}.",
        args.to, args.to
    );
}

/// Resolve a configured shard name to its primary URL.
fn resolve_shard_url(name: &str, shards: &[(String, String)]) -> Result<String, String> {
    if let Some((_, url)) = shards.iter().find(|(n, _)| n == name) {
        return Ok(url.clone());
    }
    let known: Vec<&str> = shards.iter().map(|(n, _)| n.as_str()).collect();
    Err(if known.is_empty() {
        format!("unknown shard {name:?}: no [[database.shards]] entries found")
    } else {
        format!("unknown shard {name:?}. Known shards: {}", known.join(", "))
    })
}

fn is_valid_simple_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphanumeric() && !(i == 0 && c.is_ascii_digit()))
}

/// Validate a plain or schema-qualified SQL identifier (`schema.table` or
/// `column`).  Each dot-separated part must match `[A-Za-z_][A-Za-z0-9_]*`.
/// Rejects anything else so names cannot inject SQL when interpolated.
fn validate_identifier(flag: &str, value: &str) {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.is_empty() || parts.len() > 2 {
        fail(&format!(
            "{flag} {value:?} must be a plain or schema-qualified SQL identifier \
             (e.g. `bookmarks` or `public.bookmarks`)"
        ));
    }
    for part in &parts {
        if !is_valid_simple_identifier(part) {
            fail(&format!(
                "{flag} {value:?} must be a plain or schema-qualified SQL identifier \
                 ([A-Za-z_][A-Za-z0-9_]* parts)"
            ));
        }
    }
}

/// Wrap each dot-separated part of an identifier in double quotes, producing a
/// SQL identifier that is safe for case-sensitive and schema-qualified names.
fn quote_identifier(value: &str) -> String {
    value
        .split('.')
        .map(|part| format!("\"{part}\""))
        .collect::<Vec<_>>()
        .join(".")
}

/// Build the `"key_column" = ANY(ARRAY['a','b',...]::text[])` predicate.
/// Single quotes in tenant keys are doubled for SQL safety.
fn build_key_filter(key_column: &str, tenants: &[String]) -> String {
    let list = tenants
        .iter()
        .map(|t| format!("'{}'", t.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let quoted_col = quote_identifier(key_column);
    format!("{quoted_col} = ANY(ARRAY[{list}]::text[])")
}

fn count_sql(table: &str, filter: &str) -> String {
    let qt = quote_identifier(table);
    format!("SELECT count(*) FROM {qt} WHERE {filter}")
}

/// id-independent content checksum over whole rows (`to_jsonb`), so it needs no
/// column knowledge and matches whenever source and destination hold identical
/// rows.
fn checksum_sql(table: &str, filter: &str) -> String {
    let qt = quote_identifier(table);
    format!(
        "SELECT COALESCE(md5(string_agg(j, '|' ORDER BY j)), '') \
         FROM (SELECT to_jsonb(t)::text AS j FROM {qt} t WHERE {filter}) s"
    )
}

// NOTE: `SELECT *` preserves column declaration order. Both shards are expected
// to have identical schemas (Autumn migrations run in order on all shards), so
// the order will match. If you have added columns manually outside the migration
// system, verify column order matches before using this tool.
//
// Copying the primary key is intentional: it keeps cross-table FK references
// valid after the move. If your table uses `GENERATED ALWAYS AS IDENTITY`,
// `\copy` will fail; use `GENERATED BY DEFAULT AS IDENTITY` or `BIGSERIAL`
// instead. After the copy you should reset the destination sequence:
//   SELECT setval(pg_get_serial_sequence('table', 'id_col'), MAX(id_col))
//   FROM table;
fn copy_out_sql(table: &str, filter: &str) -> String {
    let qt = quote_identifier(table);
    format!("\\copy (SELECT * FROM {qt} WHERE {filter}) TO STDOUT")
}

fn copy_in_sql(table: &str) -> String {
    let qt = quote_identifier(table);
    format!("\\copy {qt} FROM STDIN")
}

fn delete_sql(table: &str, filter: &str) -> String {
    let qt = quote_identifier(table);
    format!("DELETE FROM {qt} WHERE {filter}")
}

/// Stream rows from the source shard into the destination shard over a pipe,
/// the destination copy wrapped in a single transaction.
fn copy_rows(from_url: &str, to_url: &str, table: &str, filter: &str) {
    let mut src = Command::new("psql")
        .args([
            from_url,
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            &copy_out_sql(table, filter),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| fail(&format!("failed to start source psql: {e}")));

    let src_out = src.stdout.take().expect("piped stdout");
    let dst_status = Command::new("psql")
        .args([
            to_url,
            "-v",
            "ON_ERROR_STOP=1",
            "--single-transaction",
            "-c",
            &copy_in_sql(table),
        ])
        .stdin(src_out)
        .status()
        .unwrap_or_else(|e| fail(&format!("failed to start destination psql: {e}")));

    let src_status = src
        .wait()
        .unwrap_or_else(|e| fail(&format!("source psql did not finish: {e}")));
    if !src_status.success() || !dst_status.success() {
        fail("copy failed (see psql output above). No rows deleted.");
    }
}

/// Run a one-shot `psql -At -c <sql>` and return its single scalar value.
fn psql_scalar(url: &str, sql: &str) -> String {
    let out = Command::new("psql")
        .args([url, "-v", "ON_ERROR_STOP=1", "-At", "-c", sql])
        .output()
        .unwrap_or_else(|e| fail(&format!("failed to run psql: {e}")));
    if !out.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        fail("psql query failed (see output above)");
    }
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn snapshot(url: &str, table: &str, filter: &str) -> (String, String) {
    (
        psql_scalar(url, &count_sql(table, filter)),
        psql_scalar(url, &checksum_sql(table, filter)),
    )
}

fn run_psql(url: &str, args: &[&str]) {
    let mut full = vec![url];
    full.extend_from_slice(args);
    let status = Command::new("psql")
        .args(&full)
        .status()
        .unwrap_or_else(|e| fail(&format!("failed to run psql: {e}")));
    if !status.success() {
        fail("psql command failed (see output above)");
    }
}

fn check_psql() {
    if Command::new("psql").arg("--version").output().is_err() {
        fail(
            "`psql` not found on PATH. Install the PostgreSQL client tools to use `autumn shard move-slot`.",
        );
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("\u{2717} {msg}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shards() -> Vec<(String, String)> {
        vec![
            ("shard0".to_owned(), "postgres://h/s0".to_owned()),
            ("shard1".to_owned(), "postgres://h/s1".to_owned()),
        ]
    }

    #[test]
    fn resolve_shard_url_finds_by_name() {
        assert_eq!(
            resolve_shard_url("shard1", &shards()).unwrap(),
            "postgres://h/s1"
        );
    }

    #[test]
    fn resolve_shard_url_unknown_lists_known() {
        let err = resolve_shard_url("nope", &shards()).unwrap_err();
        assert!(err.contains("shard0") && err.contains("shard1"), "{err}");
    }

    #[test]
    fn resolve_shard_url_unknown_without_shards_explains() {
        let err = resolve_shard_url("shard0", &[]).unwrap_err();
        assert!(err.contains("no [[database.shards]]"), "{err}");
    }

    #[test]
    fn build_key_filter_quotes_column_and_escapes_values() {
        let f = build_key_filter("tenant_id", &["acme".to_owned(), "o'brien".to_owned()]);
        assert_eq!(f, "\"tenant_id\" = ANY(ARRAY['acme', 'o''brien']::text[])");
    }

    #[test]
    fn sql_builders_quote_table_and_interpolate_filter() {
        let f = build_key_filter("tenant_id", &["acme".to_owned()]);
        assert_eq!(
            count_sql("bookmarks", &f),
            "SELECT count(*) FROM \"bookmarks\" WHERE \"tenant_id\" = ANY(ARRAY['acme']::text[])"
        );
        assert!(checksum_sql("bookmarks", &f).contains("to_jsonb(t)"));
        assert_eq!(
            delete_sql("bookmarks", &f),
            "DELETE FROM \"bookmarks\" WHERE \"tenant_id\" = ANY(ARRAY['acme']::text[])"
        );
        assert!(
            copy_out_sql("bookmarks", &f).starts_with("\\copy (SELECT * FROM \"bookmarks\" WHERE")
        );
        assert_eq!(copy_in_sql("bookmarks"), "\\copy \"bookmarks\" FROM STDIN");
    }

    #[test]
    fn sql_builders_support_schema_qualified_table() {
        let f = build_key_filter("tenant_id", &["acme".to_owned()]);
        assert_eq!(
            count_sql("public.bookmarks", &f),
            "SELECT count(*) FROM \"public\".\"bookmarks\" WHERE \"tenant_id\" = ANY(ARRAY['acme']::text[])"
        );
        assert_eq!(
            copy_in_sql("public.bookmarks"),
            "\\copy \"public\".\"bookmarks\" FROM STDIN"
        );
    }

    #[test]
    fn validate_identifier_rejects_injection() {
        for ok in ["bookmarks", "tenant_id", "_t", "t1", "public.bookmarks"] {
            assert!(is_valid_schema_or_simple(ok), "{ok} should be valid");
        }
        for bad in ["", "1table", "drop table", "a;b", "a-b", "a.b.c"] {
            assert!(!is_valid_schema_or_simple(bad), "{bad} should be invalid");
        }
    }

    // Mirror of validate_identifier's logic (without process exit) so the
    // predicate is unit-testable.
    fn is_valid_schema_or_simple(value: &str) -> bool {
        let parts: Vec<&str> = value.split('.').collect();
        if parts.is_empty() || parts.len() > 2 {
            return false;
        }
        parts.iter().all(|p| is_valid_simple_identifier(p))
    }
}
