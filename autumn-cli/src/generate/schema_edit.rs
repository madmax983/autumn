//! Idempotent edits to `src/schema.rs`, `src/main.rs`, and the various
//! `mod.rs` files that the generators have to touch.
//!
//! All functions here are pure string transformations — no I/O. The
//! generator decides how to use them; the [`emit`] module decides when to
//! write them out.
//!
//! [`emit`]: super::emit

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::dsl::Field;

/// Append a `pub mod <name>;` line to a `mod.rs` file, returning the new
/// contents. Idempotent: a second call with the same name is a no-op.
#[must_use]
pub fn add_mod_declaration(existing: &str, name: &str) -> String {
    let line = format!("pub mod {name};");
    if existing
        .lines()
        .any(|l| l.trim() == line || l.trim() == format!("mod {name};"))
    {
        return existing.to_owned();
    }
    if existing.is_empty() {
        return format!("{line}\n");
    }
    let trimmed = existing.trim_end();
    format!("{trimmed}\n{line}\n")
}

/// Build a new `diesel::table!` block for the given table.
#[must_use]
pub fn schema_table_block(table: &str, fields: &[Field]) -> String {
    let mut out = String::new();
    out.push_str("diesel::table! {\n");
    let _ = writeln!(out, "    {table} (id) {{");
    out.push_str("        id -> Int8,\n");
    for f in fields {
        let _ = writeln!(out, "        {} -> {},", f.name, f.schema_type());
    }
    out.push_str("        created_at -> Timestamp,\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    out
}

/// Append a new `diesel::table!` block to `src/schema.rs`. Idempotent: if a
/// block defining `table` already exists, returns `existing` unchanged.
#[must_use]
pub fn append_schema_table(existing: &str, table: &str, fields: &[Field]) -> String {
    if has_table(existing, table) {
        return existing.to_owned();
    }
    let block = schema_table_block(table, fields);
    if existing.is_empty() {
        return block;
    }
    let trimmed = existing.trim_end();
    format!("{trimmed}\n\n{block}")
}

/// True iff `existing` already contains a `<table> (...)` definition.
fn has_table(existing: &str, table: &str) -> bool {
    let needle = format!("{table} (");
    existing.lines().any(|l| l.trim().starts_with(&needle))
}

/// Build the full SQL for `up.sql` of a `CREATE TABLE` migration with
/// optional defaults and non-unique indexes.
#[must_use]
pub fn create_table_sql_with_metadata(
    table: &str,
    fields: &[Field],
    indexes: &BTreeSet<String>,
    defaults: &BTreeMap<String, String>,
) -> String {
    let mut sql = String::with_capacity(fields.len() * 64 + indexes.len() * 96 + 128);
    let _ = writeln!(sql, "CREATE TABLE {table} (");
    sql.push_str("    id BIGSERIAL PRIMARY KEY");
    for f in fields {
        sql.push_str(",\n");
        let _ = write!(
            sql,
            "    {} {} {}",
            f.name,
            f.sql_type(),
            f.sql_nullability()
        );
        if let Some(default) = defaults.get(&f.name) {
            let _ = write!(sql, " DEFAULT {default}");
        }
    }
    sql.push_str(",\n    created_at TIMESTAMP NOT NULL DEFAULT NOW()\n);\n");
    for field_name in indexes {
        let _ = writeln!(
            sql,
            "CREATE INDEX idx_{table}_{field_name} ON {table} ({field_name});"
        );
    }
    sql
}

/// `down.sql` companion to [`create_table_sql_with_metadata`].
#[must_use]
pub fn drop_table_sql(table: &str) -> String {
    format!("DROP TABLE {table};\n")
}

/// Result of inferring a migration shape from its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationShape {
    /// `AddXxxYyyToZZZ` — emit `ALTER TABLE … ADD COLUMN` per field.
    AddColumns { table: String },
    /// `RemoveXxxYyyFromZZZ` — emit `ALTER TABLE … DROP COLUMN` per field.
    RemoveColumns { table: String },
    /// Anything else — emit empty `up.sql` / `down.sql` files.
    Empty,
}

/// Inspect a migration name (`PascalCase` from the CLI) and decide what shape
/// of SQL to emit.
#[must_use]
pub fn detect_migration_shape(pascal_name: &str) -> MigrationShape {
    if let Some(rest) = pascal_name.strip_prefix("Add")
        && let Some((_, table)) = split_on_keyword(rest, "To")
    {
        return MigrationShape::AddColumns {
            table: normalize_table_name(&table),
        };
    }
    if let Some(rest) = pascal_name.strip_prefix("Remove")
        && let Some((_, table)) = split_on_keyword(rest, "From")
    {
        return MigrationShape::RemoveColumns {
            table: normalize_table_name(&table),
        };
    }
    MigrationShape::Empty
}

/// Snake-case the supplied table name, pluralising it if it isn't already
/// plural. `Posts` → `posts`; `Post` → `posts`.
fn normalize_table_name(table_pascal: &str) -> String {
    let snake = super::naming::pascal_to_snake(table_pascal);
    if snake.ends_with('s') {
        snake
    } else {
        super::naming::pluralize(&snake)
    }
}

/// Split `XxxYyy<keyword>Zzz` into (`XxxYyy`, `Zzz`) where `<keyword>` is
/// `"To"` or `"From"` and starts a new `PascalCase` chunk.
fn split_on_keyword(s: &str, keyword: &str) -> Option<(String, String)> {
    let mut idx = 0;
    while let Some(found) = s[idx..].find(keyword) {
        let abs = idx + found;
        // Word boundary: the keyword must start at a chunk boundary
        // (the previous char must be lowercase or it's the start of the
        // string, and the char after the keyword must be uppercase).
        let prev_ok = abs == 0
            || s.as_bytes()[abs - 1].is_ascii_lowercase()
            || s.as_bytes()[abs - 1].is_ascii_digit();
        let after_idx = abs + keyword.len();
        let after_ok = s
            .as_bytes()
            .get(after_idx)
            .is_some_and(u8::is_ascii_uppercase);
        if prev_ok && after_ok {
            return Some((s[..abs].to_owned(), s[after_idx..].to_owned()));
        }
        idx = abs + 1;
    }
    None
}

/// SQL for adding columns to a table.
///
/// Prepends an `autumn-safety` comment for `NOT NULL` columns that have no
/// `DEFAULT` — those require a backfill or a default before the constraint can
/// be added safely on a live table.
#[must_use]
pub fn add_columns_up_sql(table: &str, fields: &[Field]) -> String {
    let mut out = String::new();
    for f in fields {
        if !f.nullable {
            let _ = writeln!(
                out,
                "-- autumn-safety: potentially-blocking \
                 -- add a DEFAULT or backfill existing rows before enforcing NOT NULL"
            );
        }
        let _ = writeln!(
            out,
            "ALTER TABLE {table} ADD COLUMN {} {} {};",
            f.name,
            f.sql_type(),
            f.sql_nullability()
        );
    }
    out
}

/// `down.sql` companion to [`add_columns_up_sql`].
#[must_use]
pub fn add_columns_down_sql(table: &str, fields: &[Field]) -> String {
    let mut out = String::new();
    for f in fields.iter().rev() {
        let _ = writeln!(out, "ALTER TABLE {table} DROP COLUMN {};", f.name);
    }
    out
}

/// SQL for removing columns from a table.
///
/// Prepends an `autumn-safety` comment for each `DROP COLUMN` to make the
/// rolling-deploy risk visible at a glance and machine-parseable by
/// `autumn migrate check`.
#[must_use]
pub fn remove_columns_up_sql(table: &str, fields: &[Field]) -> String {
    let mut out = String::new();
    for f in fields {
        let _ = writeln!(
            out,
            "-- autumn-safety: destructive \
             -- old replicas that reference this column will fail until restarted; \
             use expand/contract"
        );
        let _ = writeln!(out, "ALTER TABLE {table} DROP COLUMN {};", f.name);
    }
    out
}

/// `down.sql` companion to [`remove_columns_up_sql`].
#[must_use]
pub fn remove_columns_down_sql(table: &str, fields: &[Field]) -> String {
    let mut out = String::new();
    for f in fields.iter().rev() {
        let _ = writeln!(
            out,
            "ALTER TABLE {table} ADD COLUMN {} {} {};",
            f.name,
            f.sql_type(),
            f.sql_nullability()
        );
    }
    out
}

/// Add `mod <name>;` declarations to `src/main.rs` and route entries to the
/// `routes![...]` macro invocation, in a single pass.
///
/// Idempotent: existing `mod` declarations and route entries are preserved,
/// and adding the same set twice is a no-op.
#[must_use]
pub fn update_main_rs(existing: &str, mods: &[&str], route_entries: &[String]) -> String {
    let with_mods = ensure_mods(existing, mods);
    ensure_routes_entries(&with_mods, route_entries)
}

/// Insert `mod <name>;` lines near the top of `main.rs`, preserving any that
/// already exist.
///
/// ⚡ Bolt optimization: Pre-allocates string buffer based on mod count
/// and writes sequentially instead of creating intermediate vectors of strings.
fn ensure_mods(existing: &str, mods: &[&str]) -> String {
    use std::fmt::Write;
    let mut needed: Vec<&str> = mods
        .iter()
        .copied()
        .filter(|m| !has_mod_declaration(existing, m))
        .collect();
    if needed.is_empty() {
        return existing.to_owned();
    }
    needed.sort_unstable();
    let mut block = String::with_capacity(needed.len() * 15);
    for (i, m) in needed.iter().enumerate() {
        if i > 0 {
            block.push('\n');
        }
        write!(block, "mod {m};").unwrap();
    }

    // Mod declarations are *items* and must follow any crate-level inner
    // attributes (`#![allow(...)]`, `//!` doc comments) — Rust rejects the
    // file otherwise. Find the boundary between the leading attribute block
    // and the first ordinary item, and insert there.
    let split = existing
        .lines()
        .position(|l| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with("//!") && !t.starts_with("#![")
            // Inner attributes can also be written `# ! [...]` with whitespace,
            // but in practice nobody does. Stick to the canonical shape.
        })
        .unwrap_or_else(|| existing.lines().count());

    if split == 0 {
        // No leading attributes — insert at the top.
        return format!("{block}\n\n{existing}");
    }

    let mut out = String::with_capacity(existing.len() + block.len() + 4);
    let lines: Vec<&str> = existing.lines().collect();
    for line in &lines[..split] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&block);
    out.push('\n');
    if split < lines.len() {
        out.push('\n');
        for line in &lines[split..] {
            out.push_str(line);
            out.push('\n');
        }
    }
    // Preserve the original trailing-newline status.
    if !existing.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn has_mod_declaration(existing: &str, name: &str) -> bool {
    let needles = [format!("mod {name};"), format!("pub mod {name};")];
    existing
        .lines()
        .map(str::trim)
        .any(|line| needles.iter().any(|n| line == n))
}

/// Insert each entry into the body of the *first* `routes![ ... ]` macro
/// invocation. Skips entries already present.
fn ensure_routes_entries(existing: &str, entries: &[String]) -> String {
    let Some(start) = existing.find("routes![") else {
        return existing.to_owned();
    };
    let body_start = start + "routes![".len();
    // Find the matching closing bracket. The macro body cannot contain a
    // raw `]` outside of nested `[ ... ]`, so we just track depth.
    let mut depth: usize = 1;
    let bytes = existing.as_bytes();
    let mut i = body_start;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 {
        // Unmatched bracket — leave the file untouched.
        return existing.to_owned();
    }
    let body = &existing[body_start..i];
    let new_body = augment_routes_body(body, entries);
    let mut out = String::with_capacity(existing.len() + new_body.len());
    out.push_str(&existing[..body_start]);
    out.push_str(&new_body);
    out.push_str(&existing[i..]);
    out
}

fn augment_routes_body(body: &str, entries: &[String]) -> String {
    let existing_entries: Vec<String> = body
        .split([',', '\n'])
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let mut to_add: Vec<&String> = entries
        .iter()
        .filter(|e| !existing_entries.iter().any(|ex| ex == e.as_str()))
        .collect();
    if to_add.is_empty() {
        return body.to_owned();
    }
    // De-dup within `to_add` while preserving order.
    let mut seen = std::collections::HashSet::new();
    to_add.retain(|s| seen.insert(s.as_str()));

    // Detect leading whitespace inside the routes![] body so generated
    // entries match the existing indentation style.
    let indent = leading_indent(body);
    let trimmed = body.trim_end_matches([' ', '\t']);
    // Decide the insertion separator.
    let prefix = if trimmed.is_empty() || trimmed.ends_with(',') || trimmed.ends_with('\n') {
        ""
    } else {
        ","
    };
    let mut out = String::with_capacity(body.len() + to_add.len() * 32);
    out.push_str(trimmed);
    out.push_str(prefix);
    for entry in to_add {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&indent);
        out.push_str(entry);
        out.push(',');
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Return the indent (spaces/tabs) of the most-indented non-blank line in
/// `body`. Falls back to 12 spaces (the default for a `routes![]` block
/// nested inside a builder chain inside `async fn main()`).
fn leading_indent(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect::<String>()
        })
        .max_by_key(String::len)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "            ".to_owned())
}

// ── Mail preview wiring ───────────────────────────────────────────────────

/// Insert or augment a `.mail_previews(mail_previews![...])` call in the app
/// builder chain inside `src/main.rs`.
///
/// - If `mail_previews![` already exists, `mailer_type` is appended to its
///   type list (idempotent when already present).
/// - Otherwise, a new `.mail_previews(mail_previews![mailer_type])` call is
///   inserted immediately before the first `.run()` line in the builder chain.
///
/// Returns `existing` unchanged when neither injection point can be found.
#[must_use]
pub fn add_mail_preview_to_app(existing: &str, mailer_type: &str) -> String {
    const PREVIEW_MACRO: &str = "mail_previews![";
    existing.find(PREVIEW_MACRO).map_or_else(
        || insert_mail_previews_call(existing, mailer_type),
        |macro_start| {
            augment_mail_previews_list(existing, macro_start + PREVIEW_MACRO.len(), mailer_type)
        },
    )
}

/// Append `mailer_type` inside an already-present `mail_previews![...]`.
fn augment_mail_previews_list(existing: &str, body_start: usize, mailer_type: &str) -> String {
    let rest = &existing[body_start..];
    let Some(end_offset) = rest.find(']') else {
        return existing.to_owned();
    };
    let body = &rest[..end_offset];

    // Idempotency: skip if type is already registered.
    if body.split(',').map(str::trim).any(|t| t == mailer_type) {
        return existing.to_owned();
    }

    let separator = if body.trim().is_empty() { "" } else { ", " };
    let new_body = format!("{}{}{}", body.trim_end(), separator, mailer_type);
    [
        &existing[..body_start],
        &new_body,
        &existing[body_start + end_offset..],
    ]
    .concat()
}

/// Insert `.mail_previews(mail_previews![mailer_type])` before `.run()`.
fn insert_mail_previews_call(existing: &str, mailer_type: &str) -> String {
    let mut out = String::with_capacity(existing.len() + 80);
    let mut inserted = false;
    for line in existing.split('\n') {
        let trimmed = line.trim_start();
        if !inserted && trimmed.starts_with(".run()") {
            let indent_len = line.len() - trimmed.len();
            let indent = &line[..indent_len];
            out.push_str(indent);
            out.push_str(".mail_previews(mail_previews![");
            out.push_str(mailer_type);
            out.push_str("])\n");
            inserted = true;
        }
        out.push_str(line);
        out.push('\n');
    }
    // split('\n') always produces a trailing empty slice for strings ending
    // with '\n', so we have one extra '\n'. Trim it if the original didn't
    // end with a newline.
    if !existing.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    // Remove the extra trailing newline produced by the final empty segment.
    if existing.ends_with('\n') && out.ends_with("\n\n") {
        out.pop();
    }
    out
}

// ── Cargo.toml: feature injection ────────────────────────────────────────

/// Ensure the `autumn-web` dependency in `Cargo.toml` includes `feature`.
///
/// Handles the three most common forms of the dependency declaration:
///
///   1. `autumn-web = "x.y.z"` → `autumn-web = { version = "x.y.z", features = ["mail"] }`
///   2. `autumn-web = { version = "x.y.z" }` → adds `features = ["mail"]`
///   3. `autumn-web = { ..., features = ["other"] }` → appends `"mail"` to the list
///
/// Idempotent: a second call with the same feature is a no-op.
/// Returns `existing` unchanged when the `autumn-web` dep cannot be found.
#[must_use]
pub fn ensure_autumn_web_feature(existing: &str, feature: &str) -> String {
    let feature_quoted = format!("\"{feature}\"");
    let lines: Vec<&str> = existing.lines().collect();
    let mut in_deps = false;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if is_dependencies_header(trimmed) {
            in_deps = true;
            continue;
        }
        if in_deps && is_toml_table_header(trimmed) {
            in_deps = false;
            continue;
        }
        if !in_deps {
            continue;
        }
        // Only inspect lines that start with `autumn-web`.
        let after_ws = line.trim_start();
        if !after_ws.starts_with("autumn-web") {
            continue;
        }
        // Idempotency check.
        if line.contains(&feature_quoted) {
            return existing.to_owned();
        }
        let new_line = rewrite_dep_with_feature(line, feature);
        let mut out = String::with_capacity(existing.len() + 32);
        for (j, &l) in lines.iter().enumerate() {
            out.push_str(if j == i { &new_line } else { l });
            out.push('\n');
        }
        if !existing.ends_with('\n') {
            out.pop();
        }
        return out;
    }
    existing.to_owned()
}

/// Rewrite a single `autumn-web = …` TOML line to include `feature`.
fn rewrite_dep_with_feature(line: &str, feature: &str) -> String {
    let feature_quoted = format!("\"{feature}\"");
    let trimmed = line.trim();

    // Form 1: autumn-web = "x.y.z"
    if let Some(rest) = trimmed.strip_prefix("autumn-web") {
        let rest = rest.trim_start_matches([' ', '=', '\t']);
        if rest.starts_with('"')
            && let Some(version) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"'))
        {
            let indent_len = line.len() - line.trim_start().len();
            let indent = &line[..indent_len];
            return format!(
                "{indent}autumn-web = {{ version = \"{version}\", features = [{feature_quoted}] }}"
            );
        }
    }

    // Form 2/3: autumn-web = { ... features = [...] ... }
    if let Some(open) = line.find("features")
        && let Some(bracket_start) = line[open..].find('[')
    {
        let abs_start = open + bracket_start;
        if let Some(bracket_end_rel) = line[abs_start..].find(']') {
            let abs_end = abs_start + bracket_end_rel;
            let body = &line[abs_start + 1..abs_end];
            let separator = if body.trim().is_empty() { "" } else { ", " };
            return format!(
                "{}{}{}{}",
                &line[..abs_end],
                separator,
                feature_quoted,
                &line[abs_end..]
            );
        }
    }

    // Form 2b: autumn-web = { version = "x.y.z" } — no features key yet.
    // Insert features before the closing `}`.
    if let Some(close) = line.rfind('}') {
        let before = line[..close].trim_end();
        let after = &line[close..];
        return format!("{before}, features = [{feature_quoted}]{after}");
    }

    line.to_owned()
}

fn is_dependencies_header(trimmed: &str) -> bool {
    trimmed == "[dependencies]"
        || trimmed.starts_with("[dependencies]")
            && trimmed[13..].trim_start().starts_with('#')
}

fn is_toml_table_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && !trimmed.starts_with("[dependencies.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::dsl::parse_field;

    fn fields(tokens: &[&str]) -> Vec<Field> {
        tokens.iter().map(|t| parse_field(t).unwrap()).collect()
    }

    #[test]
    fn add_mod_declaration_to_empty() {
        assert_eq!(add_mod_declaration("", "post"), "pub mod post;\n");
    }

    #[test]
    fn add_mod_declaration_idempotent() {
        let initial = "pub mod post;\n";
        assert_eq!(add_mod_declaration(initial, "post"), initial);
    }

    #[test]
    fn add_mod_declaration_appends() {
        let initial = "pub mod user;\n";
        let after = add_mod_declaration(initial, "post");
        assert!(after.contains("pub mod user;"));
        assert!(after.contains("pub mod post;"));
    }

    #[test]
    fn add_mod_recognises_private_mod() {
        let initial = "mod post;\n";
        assert_eq!(add_mod_declaration(initial, "post"), initial);
    }

    #[test]
    fn schema_table_block_minimal() {
        let block = schema_table_block("posts", &fields(&["title:String"]));
        assert!(block.contains("posts (id)"));
        assert!(block.contains("id -> Int8,"));
        assert!(block.contains("title -> Text,"));
        assert!(block.contains("created_at -> Timestamp,"));
    }

    #[test]
    fn schema_table_block_nullable() {
        let block = schema_table_block("posts", &fields(&["body:Option<String>"]));
        assert!(block.contains("body -> Nullable<Text>,"));
    }

    #[test]
    fn append_schema_table_idempotent() {
        let f = fields(&["title:String"]);
        let first = append_schema_table("", "posts", &f);
        let second = append_schema_table(&first, "posts", &f);
        assert_eq!(first, second);
    }

    #[test]
    fn append_schema_table_to_existing_keeps_old() {
        let f1 = fields(&["title:String"]);
        let f2 = fields(&["name:String"]);
        let first = append_schema_table("", "posts", &f1);
        let combined = append_schema_table(&first, "users", &f2);
        assert!(combined.contains("posts (id)"));
        assert!(combined.contains("users (id)"));
    }

    #[test]
    fn create_table_sql_minimal() {
        let sql = create_table_sql_with_metadata(
            "posts",
            &fields(&["title:String"]),
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert!(sql.contains("CREATE TABLE posts ("));
        assert!(sql.contains("id BIGSERIAL PRIMARY KEY"));
        assert!(sql.contains("title TEXT NOT NULL"));
        assert!(sql.contains("created_at TIMESTAMP NOT NULL DEFAULT NOW()"));
    }

    #[test]
    fn create_table_sql_no_extra_fields() {
        let sql =
            create_table_sql_with_metadata("widgets", &[], &BTreeSet::new(), &BTreeMap::new());
        assert!(sql.contains("id BIGSERIAL PRIMARY KEY"));
        assert!(sql.contains("created_at"));
    }

    #[test]
    fn create_table_sql_nullable() {
        let sql = create_table_sql_with_metadata(
            "posts",
            &fields(&["body:Option<Text>"]),
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert!(sql.contains("body TEXT NULL"));
    }

    #[test]
    fn drop_table_sql_simple() {
        assert_eq!(drop_table_sql("posts"), "DROP TABLE posts;\n");
    }

    #[test]
    fn detect_add_migration() {
        match detect_migration_shape("AddTitleToPosts") {
            MigrationShape::AddColumns { table } => assert_eq!(table, "posts"),
            other => panic!("expected AddColumns, got {other:?}"),
        }
    }

    #[test]
    fn detect_add_migration_pluralises_singular_subject() {
        match detect_migration_shape("AddBodyToPost") {
            MigrationShape::AddColumns { table } => assert_eq!(table, "posts"),
            other => panic!("expected AddColumns, got {other:?}"),
        }
    }

    #[test]
    fn detect_remove_migration() {
        match detect_migration_shape("RemoveBodyFromPosts") {
            MigrationShape::RemoveColumns { table } => assert_eq!(table, "posts"),
            other => panic!("expected RemoveColumns, got {other:?}"),
        }
    }

    #[test]
    fn detect_other_migration_is_empty() {
        assert!(matches!(
            detect_migration_shape("BackfillSomething"),
            MigrationShape::Empty
        ));
    }

    #[test]
    fn detect_does_not_match_partial_keyword() {
        // `Tooling` should not match the `To` keyword since `o` after `To` is lowercase.
        assert!(matches!(
            detect_migration_shape("AddToolingForBuilds"),
            MigrationShape::Empty
        ));
    }

    #[test]
    fn add_columns_up_sql_emits_alter_per_field() {
        let f = fields(&["title:String", "count:i32"]);
        let sql = add_columns_up_sql("posts", &f);
        assert!(sql.contains("ALTER TABLE posts ADD COLUMN title TEXT NOT NULL;"));
        assert!(sql.contains("ALTER TABLE posts ADD COLUMN count INTEGER NOT NULL;"));
    }

    #[test]
    fn add_columns_up_sql_includes_safety_comment_for_not_null() {
        let f = fields(&["title:String"]);
        let sql = add_columns_up_sql("posts", &f);
        assert!(
            sql.contains("autumn-safety: potentially-blocking"),
            "NOT NULL column must carry a safety comment; got:\n{sql}"
        );
    }

    #[test]
    fn add_columns_up_sql_no_safety_comment_for_nullable() {
        let f = fields(&["subtitle:Option<String>"]);
        let sql = add_columns_up_sql("posts", &f);
        assert!(
            !sql.contains("autumn-safety"),
            "nullable column must NOT carry a safety comment; got:\n{sql}"
        );
    }

    #[test]
    fn remove_columns_up_sql_includes_safety_comment() {
        let f = fields(&["body:String"]);
        let sql = remove_columns_up_sql("posts", &f);
        assert!(
            sql.contains("autumn-safety: destructive"),
            "DROP COLUMN must carry a safety comment; got:\n{sql}"
        );
        assert!(sql.contains("ALTER TABLE posts DROP COLUMN body;"));
    }

    #[test]
    fn add_columns_down_sql_drops_in_reverse() {
        let f = fields(&["title:String", "count:i32"]);
        let sql = add_columns_down_sql("posts", &f);
        let title_pos = sql.find("DROP COLUMN title").unwrap();
        let count_pos = sql.find("DROP COLUMN count").unwrap();
        assert!(count_pos < title_pos);
    }

    #[test]
    fn update_main_rs_inserts_mod_and_routes() {
        let original = r#"use autumn_web::prelude::*;

#[get("/")]
async fn index() -> &'static str { "ok" }

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index])
        .run()
        .await;
}
"#;
        let updated = update_main_rs(
            original,
            &["models", "routes", "schema"],
            &["routes::posts::index".to_owned()],
        );
        assert!(updated.contains("mod models;"));
        assert!(updated.contains("mod routes;"));
        assert!(updated.contains("mod schema;"));
        assert!(updated.contains("routes::posts::index"));
        assert!(updated.contains("index,")); // original entry preserved
    }

    #[test]
    fn update_main_rs_preserves_inner_attributes() {
        // Inserting `mod` items above `#![...]` would make the file reject —
        // crate-level inner attributes must precede every item.
        let original = "#![allow(clippy::needless_pass_by_value)]\n\
#![deny(unsafe_code)]\n\
\n\
use autumn_web::prelude::*;\n\
\n\
#[autumn_web::main]\n\
async fn main() {\n\
    autumn_web::app().run().await;\n\
}\n";
        let updated = update_main_rs(original, &["models"], &[]);
        let attr_pos = updated.find("#![allow").unwrap();
        let mod_pos = updated.find("mod models;").unwrap();
        assert!(
            attr_pos < mod_pos,
            "crate inner attributes must stay above mod items:\n{updated}"
        );
        assert!(updated.contains("#![deny(unsafe_code)]"));
    }

    #[test]
    fn update_main_rs_inserts_after_doc_comment_block() {
        let original = "//! Top-level docs.\n\
//! Continuation.\n\
\n\
use autumn_web::prelude::*;\n";
        let updated = update_main_rs(original, &["models"], &[]);
        let docs_pos = updated.find("//! Top-level docs.").unwrap();
        let mod_pos = updated.find("mod models;").unwrap();
        assert!(docs_pos < mod_pos);
    }

    #[test]
    fn update_main_rs_idempotent() {
        let original = "mod models;\n\
mod routes;\n\
mod schema;\n\
\n\
use autumn_web::prelude::*;\n\
\n\
#[autumn_web::main]\n\
async fn main() {\n\
    autumn_web::app()\n\
        .routes(routes![\n\
            routes::posts::index,\n\
        ])\n\
        .run()\n\
        .await;\n\
}\n";
        let once = update_main_rs(
            original,
            &["models", "routes", "schema"],
            &["routes::posts::index".to_owned()],
        );
        let twice = update_main_rs(
            &once,
            &["models", "routes", "schema"],
            &["routes::posts::index".to_owned()],
        );
        assert_eq!(once, twice);
    }

    #[test]
    fn update_main_rs_no_routes_macro_leaves_file_alone() {
        let original = "fn main() {}\n";
        let updated = update_main_rs(original, &[], &["foo".into()]);
        assert_eq!(updated, original);
    }

    #[test]
    fn ensure_routes_entries_handles_empty_body() {
        let original = "fn main() {\n    routes![]\n}\n";
        let updated = ensure_routes_entries(original, &["foo".into()]);
        assert!(updated.contains("foo"));
    }

    // ── add_mail_preview_to_app ───────────────────────────────────────────

    fn app_main() -> &'static str {
        "use autumn_web::prelude::*;\n\
         \n\
         #[autumn_web::main]\n\
         async fn main() {\n\
             autumn_web::app()\n\
                 .routes(routes![index])\n\
                 .run()\n\
                 .await;\n\
         }\n"
    }

    #[test]
    fn add_mail_preview_inserts_before_run() {
        let updated = add_mail_preview_to_app(app_main(), "mailers::welcome::WelcomeMailer");
        assert!(
            updated.contains("mail_previews![mailers::welcome::WelcomeMailer]"),
            "must insert mail_previews call: {updated}"
        );
        let preview_pos = updated.find("mail_previews").unwrap();
        let run_pos = updated.find(".run()").unwrap();
        assert!(
            preview_pos < run_pos,
            "mail_previews must appear before .run(): {updated}"
        );
    }

    #[test]
    fn add_mail_preview_idempotent() {
        let first =
            add_mail_preview_to_app(app_main(), "mailers::welcome::WelcomeMailer");
        let second =
            add_mail_preview_to_app(&first, "mailers::welcome::WelcomeMailer");
        assert_eq!(first, second, "second call must be a no-op");
    }

    #[test]
    fn add_mail_preview_augments_existing_call() {
        let after_first =
            add_mail_preview_to_app(app_main(), "mailers::welcome::WelcomeMailer");
        let after_second =
            add_mail_preview_to_app(&after_first, "mailers::notify::NotifyMailer");
        assert!(after_second.contains("mailers::welcome::WelcomeMailer"));
        assert!(after_second.contains("mailers::notify::NotifyMailer"));
        assert_eq!(
            after_second.matches("mail_previews![").count(),
            1,
            "must not duplicate the mail_previews![] call: {after_second}"
        );
    }

    #[test]
    fn add_mail_preview_preserves_run_await() {
        let updated =
            add_mail_preview_to_app(app_main(), "mailers::welcome::WelcomeMailer");
        assert!(updated.contains(".run()"), ".run() must still be present");
        assert!(updated.contains(".await;"), ".await must still be present");
    }

    // ── ensure_autumn_web_feature ─────────────────────────────────────────

    #[test]
    fn ensure_feature_transforms_bare_string_dep() {
        let cargo =
            "[package]\nname=\"x\"\n\n[dependencies]\nautumn-web = \"0.6\"\n";
        let updated = ensure_autumn_web_feature(cargo, "mail");
        assert!(
            updated.contains("\"mail\""),
            "must add mail feature: {updated}"
        );
        assert!(
            updated.contains("version"),
            "must preserve version: {updated}"
        );
    }

    #[test]
    fn ensure_feature_idempotent_bare_string() {
        let cargo =
            "[package]\nname=\"x\"\n\n[dependencies]\nautumn-web = \"0.6\"\n";
        let once = ensure_autumn_web_feature(cargo, "mail");
        let twice = ensure_autumn_web_feature(&once, "mail");
        assert_eq!(once, twice, "second call must be a no-op");
    }

    #[test]
    fn ensure_feature_adds_to_existing_features_list() {
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\n\
                     autumn-web = { version = \"0.6\", features = [\"db\"] }\n";
        let updated = ensure_autumn_web_feature(cargo, "mail");
        assert!(updated.contains("\"mail\""));
        assert!(updated.contains("\"db\""), "must preserve existing feature");
    }

    #[test]
    fn ensure_feature_adds_features_key_when_absent() {
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\n\
                     autumn-web = { version = \"0.6\" }\n";
        let updated = ensure_autumn_web_feature(cargo, "mail");
        assert!(
            updated.contains("\"mail\""),
            "must add features key: {updated}"
        );
    }

    #[test]
    fn ensure_feature_idempotent_inline_table() {
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\n\
                     autumn-web = { version = \"0.6\", features = [\"mail\"] }\n";
        let updated = ensure_autumn_web_feature(cargo, "mail");
        assert_eq!(cargo, updated, "already-present feature must be a no-op");
    }

    #[test]
    fn ensure_feature_ignores_unrelated_deps() {
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\n\
                     serde = \"1\"\nautumn-web = \"0.6\"\ntracing = \"0.1\"\n";
        let updated = ensure_autumn_web_feature(cargo, "mail");
        assert!(updated.contains("serde = \"1\""), "unrelated dep must be preserved");
        assert!(updated.contains("\"mail\""));
    }
}
