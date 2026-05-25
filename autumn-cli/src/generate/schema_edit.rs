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
    /// `AddSearchTo<Table>` or `AddSearchableTo<Table>` or `AddSearchVectorTo<Table>`
    AddSearch { table: String },
    /// Anything else — emit empty `up.sql` / `down.sql` files.
    Empty,
}

/// Inspect a migration name (`PascalCase` from the CLI) and decide what shape
/// of SQL to emit.
#[must_use]
pub fn detect_migration_shape(pascal_name: &str) -> MigrationShape {
    if let Some(rest) = pascal_name.strip_prefix("AddSearchTo")
        && rest.chars().next().is_some_and(char::is_uppercase)
    {
        return MigrationShape::AddSearch {
            table: normalize_table_name(rest),
        };
    }
    if let Some(rest) = pascal_name.strip_prefix("AddSearchableTo")
        && rest.chars().next().is_some_and(char::is_uppercase)
    {
        return MigrationShape::AddSearch {
            table: normalize_table_name(rest),
        };
    }
    if let Some(rest) = pascal_name.strip_prefix("AddSearchVectorTo")
        && rest.chars().next().is_some_and(char::is_uppercase)
    {
        return MigrationShape::AddSearch {
            table: normalize_table_name(rest),
        };
    }

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
        || trimmed.starts_with("[dependencies]") && trimmed[13..].trim_start().starts_with('#')
}

fn is_toml_table_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && !trimmed.starts_with("[dependencies.")
}

/// SQL for adding a stored generated `search_vector` column and GIN index.
#[must_use]
pub fn add_search_up_sql(table: &str, language: &str, fields: &[(String, char)]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- autumn-safety: potentially-blocking \n\
         -- adding stored generated column will backfill existing rows"
    );

    let mut expr = String::new();
    for (i, (field, weight)) in fields.iter().enumerate() {
        if i > 0 {
            expr.push_str(" || ");
        }
        let _ = write!(
            expr,
            "setweight(to_tsvector('{language}'::regconfig, coalesce({field}::text, '')), '{weight}')"
        );
    }

    let _ = writeln!(
        out,
        "ALTER TABLE {table} ADD COLUMN search_vector tsvector GENERATED ALWAYS AS ({expr}) STORED;"
    );
    let _ = writeln!(
        out,
        "CREATE INDEX idx_{table}_search_vector ON {table} USING gin(search_vector);"
    );
    out
}

/// `down.sql` companion to [`add_search_up_sql`].
#[must_use]
pub fn add_search_down_sql(table: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "DROP INDEX IF EXISTS idx_{table}_search_vector;");
    let _ = writeln!(
        out,
        "ALTER TABLE {table} DROP COLUMN IF EXISTS search_vector;"
    );
    out
}

#[allow(clippy::option_if_let_else, clippy::too_many_lines)]
pub fn singularize(s: &str) -> String {
    if s == "series" {
        return "series".to_string();
    }
    if let Some(stripped) = s.strip_suffix("people") {
        return format!("{stripped}person");
    }
    if let Some(stripped) = s.strip_suffix("children") {
        return format!("{stripped}child");
    }

    let is_false_men = s == "specimen"
        || s == "regimen"
        || s == "abdomen"
        || s == "lumen"
        || s == "omen"
        || s == "semen"
        || s == "hymen"
        || s == "acumen"
        || s == "bitumen"
        || s == "stamen"
        || s.ends_with("specimen")
        || s.ends_with("regimen")
        || s.ends_with("abdomen")
        || s.ends_with("lumen")
        || s.ends_with("omen")
        || s.ends_with("semen")
        || s.ends_with("hymen")
        || s.ends_with("acumen")
        || s.ends_with("bitumen")
        || s.ends_with("stamen");

    if s == "men" {
        return "man".to_string();
    }
    if s == "women" {
        return "woman".to_string();
    }
    if s.ends_with("men") && !is_false_men {
        let stripped = s.strip_suffix("men").unwrap();
        return format!("{stripped}man");
    }
    if s.ends_with("women") {
        let stripped = s.strip_suffix("women").unwrap();
        return format!("{stripped}woman");
    }

    if let Some(stripped) = s.strip_suffix("ies") {
        if s.ends_with("movies") || s.ends_with("cookies") || s.ends_with("zombies") {
            format!("{stripped}ie")
        } else {
            format!("{stripped}y")
        }
    } else if let Some(stripped) = s.strip_suffix("es") {
        if s.ends_with("ches")
            || s.ends_with("shes")
            || s.ends_with("xes")
            || s.ends_with("ses")
            || s.ends_with("zes")
        {
            if s.ends_with("statuses")
                || s.ends_with("aliases")
                || s.ends_with("buses")
                || s.ends_with("sses")
                || s.ends_with("lenses")
            {
                stripped.to_owned()
            } else if s.ends_with("yses") {
                format!("{stripped}is")
            } else if s == "crises" {
                "crisis".to_string()
            } else if s == "diagnoses" {
                "diagnosis".to_string()
            } else if s == "neuroses" {
                "neurosis".to_string()
            } else if s == "bases" {
                "basis".to_string()
            } else if s == "oases" {
                "oasis".to_string()
            } else if s.ends_with("ases")
                || s.ends_with("ises")
                || s.ends_with("oses")
                || s.ends_with("uses")
                || s.ends_with("ses")
            {
                format!("{stripped}e")
            } else {
                stripped.to_owned()
            }
        } else {
            format!("{stripped}e")
        }
    } else if let Some(stripped) = s.strip_suffix('s') {
        if s.ends_with("ss")
            || s == "news"
            || s == "status"
            || s == "alias"
            || s == "bus"
            || s == "lens"
            || s == "virus"
            || s == "canvas"
            || s == "analysis"
            || s == "basis"
            || s == "crisis"
        {
            s.to_owned()
        } else {
            stripped.to_owned()
        }
    } else {
        s.to_owned()
    }
}

fn strip_comments(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut result = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        // 1. Check for single-line comment
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            if i < chars.len() {
                result.push('\n');
                i += 1;
            } else {
                result.push(' ');
            }
            continue;
        }

        // 2. Check for block comment
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
            i += 2;
            let mut depth = 1;
            while i + 1 < chars.len() && depth > 0 {
                if chars[i] == '/' && chars[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                i = chars.len();
            }
            result.push(' ');
            continue;
        }

        // 3. Check for raw string literal: r"..." or r#"..."# or r##"..."##
        if chars[i] == 'r' && i + 1 < chars.len() {
            let mut hash_count = 0;
            let mut j = i + 1;
            while j < chars.len() && chars[j] == '#' {
                hash_count += 1;
                j += 1;
            }
            if j < chars.len() && chars[j] == '"' {
                result.extend(&chars[i..=j]);
                i = j + 1;

                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '"' {
                        let mut match_hashes = true;
                        for h in 0..hash_count {
                            if i + 1 + h >= chars.len() || chars[i + 1 + h] != '#' {
                                match_hashes = false;
                                break;
                            }
                        }
                        if match_hashes {
                            result.push('"');
                            for _ in 0..hash_count {
                                result.push('#');
                            }
                            i += 1 + hash_count;
                            closed = true;
                            break;
                        }
                    }
                    result.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    i = chars.len();
                }
                continue;
            }
        }

        // 4. Check for standard double-quoted string
        if chars[i] == '"' {
            result.push('"');
            i += 1;
            while i < chars.len() {
                let ch = chars[i];
                result.push(ch);
                if ch == '\\' && i + 1 < chars.len() {
                    result.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if ch == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // 5. Normal character
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Scan a model file content to extract the `#[searchable]` language and field weights.
#[must_use]
#[allow(dead_code)]
pub fn parse_model_search_config(content: &str) -> Option<(String, Vec<(String, char)>)> {
    parse_model_search_config_for_table(content, "")
}

/// Scan a model file content to extract the `#[searchable]` language and field weights for a specific table.
fn is_matching_table_attr(attr_content: &str, table: &str) -> bool {
    let mut rest = attr_content;
    while let Some(pos) = rest.find("table") {
        let prev_char = if pos > 0 {
            rest.as_bytes().get(pos - 1)
        } else {
            None
        };
        let next_char = rest.as_bytes().get(pos + "table".len());
        let is_prev_boundary = prev_char.is_none_or(|&c| !c.is_ascii_alphanumeric() && c != b'_');
        let is_next_boundary = next_char.is_none_or(|&c| !c.is_ascii_alphanumeric() && c != b'_');
        if is_prev_boundary && is_next_boundary {
            let after_table = &rest[pos + "table".len()..];
            let trimmed = after_table.trim_start();
            if let Some(stripped_eq) = trimmed.strip_prefix('=') {
                let after_eq = stripped_eq.trim_start();

                // 1. Try normal string literal
                let expected_value = format!("\"{table}\"");
                if after_eq.starts_with(&expected_value) {
                    return true;
                }

                // 2. Try raw string literal (e.g. r"table" or r#"table"#)
                if let Some(after_r) = after_eq.strip_prefix('r') {
                    let mut hash_count = 0;
                    let bytes = after_r.as_bytes();
                    while hash_count < bytes.len() && bytes[hash_count] == b'#' {
                        hash_count += 1;
                    }
                    let after_hashes = &after_r[hash_count..];
                    let expected_raw = format!("\"{table}\"");
                    if after_hashes.starts_with(&expected_raw) {
                        let after_quote = &after_hashes[expected_raw.len()..];
                        let mut match_close = true;
                        for h in 0..hash_count {
                            if after_quote.as_bytes().get(h) != Some(&b'#') {
                                match_close = false;
                                break;
                            }
                        }
                        if match_close {
                            return true;
                        }
                    }
                }
            }
        }
        rest = &rest[pos + "table".len()..];
    }
    false
}

#[allow(clippy::collapsible_if)]
fn extract_diesel_column_name(attr: &str) -> Option<String> {
    let pos = attr.find("column_name")?;
    let after_col = &attr[pos + "column_name".len()..];
    let trimmed = after_col.trim_start();
    let stripped_eq = trimmed.strip_prefix('=')?;
    let after_eq = stripped_eq.trim_start();

    // 1. Try standard double quotes
    if let Some(stripped_quote) = after_eq.strip_prefix('"') {
        let quote_end = stripped_quote.find('"')?;
        return Some(stripped_quote[..quote_end].to_string());
    }

    // 2. Try raw string literal e.g. r#"headline"# or r"headline"
    if let Some(after_r) = after_eq.strip_prefix('r') {
        let mut hash_count = 0;
        let bytes = after_r.as_bytes();
        while hash_count < bytes.len() && bytes[hash_count] == b'#' {
            hash_count += 1;
        }
        let after_hashes = &after_r[hash_count..];
        if let Some(stripped_quote) = after_hashes.strip_prefix('"') {
            if let Some(quote_end) = stripped_quote.find('"') {
                return Some(stripped_quote[..quote_end].to_string());
            }
        }
    }

    // 3. Try unquoted identifier form e.g. column_name = headline or column_name = r#type
    let after_ident = after_eq.strip_prefix("r#").unwrap_or(after_eq);

    let mut id_chars = String::new();
    for c in after_ident.chars() {
        if c.is_alphanumeric() || c == '_' {
            id_chars.push(c);
        } else {
            break;
        }
    }
    if !id_chars.is_empty() {
        return Some(id_chars);
    }

    None
}

fn has_attribute_boundary(rest: &str, pos: usize, keyword: &str) -> bool {
    let after = &rest[pos + keyword.len()..];
    after
        .chars()
        .next()
        .is_none_or(|c| c == '(' || c == ']' || c.is_whitespace())
}

#[must_use]
#[allow(clippy::too_many_lines, clippy::collapsible_if)]
pub fn parse_model_search_config_for_table(
    content: &str,
    table: &str,
) -> Option<(String, Vec<(String, char)>)> {
    let clean_content = strip_comments(content);
    let mut language = "simple".to_string();
    let mut fields = Vec::new();

    // 1. Locate the model struct position anchored by #[model] or #[autumn_web::model] for the given table
    let mut model_pos = None;
    let mut struct_pos = None;

    if !table.is_empty() {
        // Try to find #[model(...table = "table"...)]
        let mut rest = clean_content.as_str();
        while let Some(pos) = rest.find("#[model") {
            let offset = clean_content.len() - rest.len() + pos;
            if has_attribute_boundary(rest, pos, "#[model") {
                if let Some(close_bracket) = rest[pos..].find(']') {
                    let attr_content = &rest[pos..pos + close_bracket];
                    if is_matching_table_attr(attr_content, table) {
                        model_pos = Some(offset);
                        break;
                    }
                }
            }
            rest = &rest[pos + "#[model".len()..];
        }

        if model_pos.is_none() {
            let mut rest = clean_content.as_str();
            while let Some(pos) = rest.find("#[autumn_web::model") {
                let offset = clean_content.len() - rest.len() + pos;
                if has_attribute_boundary(rest, pos, "#[autumn_web::model") {
                    if let Some(close_bracket) = rest[pos..].find(']') {
                        let attr_content = &rest[pos..pos + close_bracket];
                        if is_matching_table_attr(attr_content, table) {
                            model_pos = Some(offset);
                            break;
                        }
                    }
                }
                rest = &rest[pos + "#[autumn_web::model".len()..];
            }
        }

        // Try PascalCase struct name fallback
        if model_pos.is_none() {
            let singular = singularize(table);
            let struct_name = super::naming::snake_to_pascal(&singular);

            let mut search_rest = clean_content.as_str();
            let mut found_struct_pos = None;
            while let Some(pos) = search_rest.find("struct ") {
                let offset = clean_content.len() - search_rest.len() + pos;
                let after_struct = &search_rest[pos + "struct ".len()..];
                if let Some(first_word) = after_struct.split_whitespace().next() {
                    let clean_name =
                        first_word.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
                    if clean_name == struct_name {
                        found_struct_pos = Some(offset);
                        break;
                    }
                }
                search_rest = &search_rest[pos + "struct ".len()..];
            }

            if let Some(s_pos) = found_struct_pos {
                let before_struct = &clean_content[..s_pos];

                let mut m_pos_opt = None;
                let mut rest_before = before_struct;
                while let Some(p) = rest_before.rfind("#[model") {
                    if has_attribute_boundary(rest_before, p, "#[model") {
                        m_pos_opt = Some(p);
                        break;
                    }
                    rest_before = &rest_before[..p];
                }

                let mut aw_pos_opt = None;
                let mut rest_before_aw = before_struct;
                while let Some(p) = rest_before_aw.rfind("#[autumn_web::model") {
                    if has_attribute_boundary(rest_before_aw, p, "#[autumn_web::model") {
                        aw_pos_opt = Some(p);
                        break;
                    }
                    rest_before_aw = &rest_before_aw[..p];
                }

                let best_pos = match (m_pos_opt, aw_pos_opt) {
                    (Some(p1), Some(p2)) => Some(std::cmp::max(p1, p2)),
                    (Some(p), None) | (None, Some(p)) => Some(p),
                    (None, None) => None,
                };

                if let Some(pos) = best_pos {
                    let in_between = &before_struct[pos..];
                    let has_other_struct = in_between.find("struct").is_some_and(|idx| {
                        let bytes = in_between.as_bytes();
                        let next_char = bytes.get(idx + "struct".len());
                        next_char.is_none_or(|&c| c.is_ascii_whitespace())
                    });
                    if !has_other_struct {
                        model_pos = Some(pos);
                        struct_pos = Some(s_pos);
                    }
                }
            }
        }
    }

    if model_pos.is_none() {
        if table.is_empty() {
            let mut rest = clean_content.as_str();
            while let Some(pos) = rest.find("#[model") {
                if has_attribute_boundary(rest, pos, "#[model") {
                    model_pos = Some(clean_content.len() - rest.len() + pos);
                    break;
                }
                rest = &rest[pos + "#[model".len()..];
            }
            if model_pos.is_none() {
                let mut rest = clean_content.as_str();
                while let Some(pos) = rest.find("#[autumn_web::model") {
                    if has_attribute_boundary(rest, pos, "#[autumn_web::model") {
                        model_pos = Some(clean_content.len() - rest.len() + pos);
                        break;
                    }
                    rest = &rest[pos + "#[autumn_web::model".len()..];
                }
            }
        } else {
            return None;
        }
    }

    let struct_pos = if let Some(s_pos) = struct_pos {
        s_pos
    } else if let Some(m_pos) = model_pos {
        if let Some(struct_offset) = clean_content[m_pos..].find("struct ") {
            m_pos + struct_offset
        } else {
            return None;
        }
    } else {
        clean_content.find("struct ")?
    };

    // 2. Restrict FTS language search to the struct-level #[searchable] attribute (preceding our struct)
    let before_struct = &clean_content[..struct_pos];
    let mut rest_before = before_struct;
    while let Some(pos) = rest_before.rfind("#[searchable") {
        let next_char = rest_before.as_bytes().get(pos + "#[searchable".len());
        let is_boundary =
            next_char.is_none_or(|&c| c == b']' || c == b'(' || c.is_ascii_whitespace());
        if !is_boundary {
            rest_before = &rest_before[..pos];
            continue;
        }
        let attr_chunk = &rest_before[pos..];
        if let Some(close_bracket) = attr_chunk.find(']') {
            let attr_content = &attr_chunk[..close_bracket];
            if let Some(lang_pos) = attr_content.find("language") {
                let after_lang = &attr_content[lang_pos + "language".len()..];
                if let Some(eq_pos) = after_lang.find('=') {
                    let after_eq = &after_lang[eq_pos + 1..];
                    if let Some(quote_start) = after_eq.find('"') {
                        let after_quote = &after_eq[quote_start + 1..];
                        if let Some(quote_end) = after_quote.find('"') {
                            language = after_quote[..quote_end].to_string();
                        }
                    }
                }
            }
        }
        break;
    }

    // 3. Extract the target model's struct body definition by matching structural braces
    let mut struct_body = "";
    if let Some(open_brace_offset) = clean_content[struct_pos..].find('{') {
        let open_brace_pos = struct_pos + open_brace_offset;
        let chars: Vec<char> = clean_content[open_brace_pos + 1..].chars().collect();
        let mut brace_count = 1;
        let mut close_brace_offset = None;
        let mut i = 0;
        while i < chars.len() {
            // Check for raw string literal: r"..." or r#"..."#
            if chars[i] == 'r' && i + 1 < chars.len() {
                let mut hash_count = 0;
                let mut j = i + 1;
                while j < chars.len() && chars[j] == '#' {
                    hash_count += 1;
                    j += 1;
                }
                if j < chars.len() && chars[j] == '"' {
                    i = j + 1;
                    while i < chars.len() {
                        if chars[i] == '"' {
                            let mut match_hashes = true;
                            for h in 0..hash_count {
                                if i + 1 + h >= chars.len() || chars[i + 1 + h] != '#' {
                                    match_hashes = false;
                                    break;
                                }
                            }
                            if match_hashes {
                                i += 1 + hash_count;
                                break;
                            }
                        }
                        i += 1;
                    }
                    continue;
                }
            }

            // Check for standard double-quoted string
            if chars[i] == '"' {
                i += 1;
                while i < chars.len() {
                    let ch = chars[i];
                    if ch == '\\' && i + 1 < chars.len() {
                        i += 2;
                        continue;
                    }
                    if ch == '"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }

            // Check for structural braces
            if chars[i] == '{' {
                brace_count += 1;
            } else if chars[i] == '}' {
                brace_count -= 1;
                if brace_count == 0 {
                    close_brace_offset = Some(i);
                    break;
                }
            }
            i += 1;
        }

        if let Some(offset) = close_brace_offset {
            let consumed: String = chars[..offset].iter().collect();
            let consumed_len = consumed.len();
            struct_body = &clean_content[open_brace_pos + 1..open_brace_pos + 1 + consumed_len];
        }
    }

    // 4. Restrict FTS fields loop to scan only inside the struct body
    let mut rest = struct_body;
    while let Some(pos) = rest.find("#[searchable") {
        // Enforce word boundaries on the #[searchable] prefix check
        let next_char = rest.as_bytes().get(pos + "#[searchable".len());
        let is_boundary =
            next_char.is_none_or(|&c| c == b']' || c == b'(' || c.is_ascii_whitespace());
        if !is_boundary {
            rest = &rest[pos + "#[searchable".len()..];
            continue;
        }

        let attr_chunk = &rest[pos..];
        let mut weight = 'D';

        if let Some(close_bracket) = attr_chunk.find(']') {
            let attr_content = &attr_chunk[..close_bracket];
            // Restrict weight search purely to the current attribute block contents
            if let Some(w_pos) = attr_content.find("weight") {
                let after_weight = &attr_content[w_pos + "weight".len()..];
                if let Some(eq_pos) = after_weight.find('=') {
                    let after_eq = &after_weight[eq_pos + 1..];
                    if let Some(quote_start) = after_eq.find('"') {
                        let after_quote = &after_eq[quote_start + 1..];
                        if let Some(quote_end) = after_quote.find('"') {
                            let w_str = &after_quote[..quote_end];
                            if let Some(ch) = w_str.chars().next() {
                                let upper = ch.to_ascii_uppercase();
                                if upper == 'A' || upper == 'B' || upper == 'C' || upper == 'D' {
                                    weight = upper;
                                } else {
                                    weight = 'D';
                                }
                            }
                        }
                    }
                }
            }

            let after_attr = &attr_chunk[close_bracket + 1..];
            let mut line_to_parse = "";
            let mut field_attributes = Vec::new();
            for line in after_attr.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed.starts_with("#[") {
                    field_attributes.push(trimmed);
                    continue;
                }
                line_to_parse = trimmed;
                break;
            }

            if !line_to_parse.is_empty() {
                let mut parts = line_to_parse;
                if let Some(stripped_pub) = parts.strip_prefix("pub") {
                    parts = stripped_pub.trim();
                    if let Some(stripped_paren) = parts.strip_prefix('(')
                        && let Some(close_paren) = stripped_paren.find(')')
                    {
                        parts = stripped_paren[close_paren + 1..].trim();
                    }
                }
                if let Some(colon) = parts.find(':') {
                    let field_name = parts[..colon].trim().to_string();
                    let mut clean_field = field_name.as_str();
                    if let Some(stripped) = clean_field.strip_prefix("r#") {
                        clean_field = stripped;
                    }
                    if !clean_field.is_empty()
                        && clean_field.chars().all(|c| c.is_alphanumeric() || c == '_')
                    {
                        let mut final_col_name = clean_field.to_string();

                        // 1. Scan attributes *before* #[searchable]
                        let before_searchable_attr = &rest[..pos];
                        let prev_term_pos = before_searchable_attr
                            .rfind([';', ',', '{'])
                            .map_or(0, |idx| idx + 1);
                        let field_prefix = &before_searchable_attr[prev_term_pos..];

                        if let Some(d_pos) = field_prefix.find("#[diesel") {
                            let sub_chunk = &field_prefix[d_pos..];
                            if let Some(cb) = sub_chunk.find(']') {
                                let attr_content = &sub_chunk[..cb];
                                if let Some(custom_col) = extract_diesel_column_name(attr_content) {
                                    final_col_name = custom_col;
                                }
                            }
                        }

                        // 2. Scan attributes *after* #[searchable] but before field declaration
                        for attr in &field_attributes {
                            if let Some(custom_col) = extract_diesel_column_name(attr) {
                                final_col_name = custom_col;
                                break;
                            }
                        }

                        fields.push((final_col_name, weight));
                    }
                }
            }
        }

        rest = &rest[pos + "#[searchable".len()..];
    }

    if fields.is_empty() {
        None
    } else {
        Some((language, fields))
    }
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
        let first = add_mail_preview_to_app(app_main(), "mailers::welcome::WelcomeMailer");
        let second = add_mail_preview_to_app(&first, "mailers::welcome::WelcomeMailer");
        assert_eq!(first, second, "second call must be a no-op");
    }

    #[test]
    fn add_mail_preview_augments_existing_call() {
        let after_first = add_mail_preview_to_app(app_main(), "mailers::welcome::WelcomeMailer");
        let after_second = add_mail_preview_to_app(&after_first, "mailers::notify::NotifyMailer");
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
        let updated = add_mail_preview_to_app(app_main(), "mailers::welcome::WelcomeMailer");
        assert!(updated.contains(".run()"), ".run() must still be present");
        assert!(updated.contains(".await;"), ".await must still be present");
    }

    // ── ensure_autumn_web_feature ─────────────────────────────────────────

    #[test]
    fn ensure_feature_transforms_bare_string_dep() {
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\nautumn-web = \"0.6\"\n";
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
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\nautumn-web = \"0.6\"\n";
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
        assert!(
            updated.contains("serde = \"1\""),
            "unrelated dep must be preserved"
        );
        assert!(updated.contains("\"mail\""));
    }

    #[test]
    fn ensure_feature_returns_unchanged_when_autumn_web_absent() {
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\nserde = \"1\"\n";
        let updated = ensure_autumn_web_feature(cargo, "mail");
        assert_eq!(cargo, updated, "no autumn-web dep → must be a no-op");
    }

    #[test]
    fn ensure_feature_dep_without_closing_brace_uses_fallback() {
        // Malformed line — none of the three forms match, fallback returns unchanged.
        let cargo = "[package]\nname=\"x\"\n\n[dependencies]\nautumn-web = malformed\n";
        let updated = ensure_autumn_web_feature(cargo, "mail");
        // The function should not panic; it falls back to the existing line.
        assert!(updated.contains("autumn-web = malformed"));
    }

    #[test]
    fn add_mail_preview_unclosed_bracket_returns_unchanged() {
        // Malformed source: `mail_previews![` with no closing `]`.
        let src = "app()\n    .mail_previews(mail_previews![Foo)\n    .run()\n    .await;\n";
        let updated = add_mail_preview_to_app(src, "Bar");
        // Must not panic; returns the original string unchanged.
        assert_eq!(src, updated);
    }

    #[test]
    fn add_mail_preview_no_run_returns_string_with_preview_appended() {
        // Source with no `.run()` call — insertion is skipped, function still returns.
        let src = "app()\n    .routes(routes![index])\n";
        let updated = add_mail_preview_to_app(src, "mailers::welcome::WelcomeMailer");
        // No `.run()` means we can't find an insertion point; original is returned.
        assert!(
            !updated.contains("mail_previews"),
            "no insertion point → no insertion"
        );
    }

    #[test]
    fn test_singularize_simple() {
        assert_eq!(singularize("posts"), "post");
        assert_eq!(singularize("categories"), "category");
        assert_eq!(singularize("wishes"), "wish");
        assert_eq!(singularize("test_search_records"), "test_search_record");
    }

    #[test]
    fn test_parse_model_search_config_simple() {
        let content = r#"
#[autumn_web::model(table = "test_search_records")]
#[searchable(language = "english")]
#[derive(PartialEq, Eq)]
pub struct SearchRecord {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        let (lang, fields) = parse_model_search_config(content).unwrap();
        assert_eq!(lang, "english");
        assert_eq!(
            fields,
            vec![("title".to_string(), 'A'), ("body".to_string(), 'B'),]
        );
    }

    #[test]
    fn test_parse_model_search_config_robustness() {
        // 1. Check space-less language parsing
        let content_spaceless = r#"
#[autumn_web::model(table = "test_search_records")]
#[searchable(language="english")]
pub struct SearchRecord {
    #[id]
    pub id: i64,
    #[searchable]
    pub title: String,
}
"#;
        let (lang, fields) = parse_model_search_config(content_spaceless).unwrap();
        assert_eq!(lang, "english");
        assert_eq!(fields, vec![("title".to_string(), 'D')]);

        // 2. Check unweighted vs weighted weight inheritance leakage
        let content_leakage = r#"
#[autumn_web::model(table = "test_search_records")]
#[searchable(language = "simple")]
pub struct SearchRecord {
    #[id]
    pub id: i64,
    #[searchable]
    pub title: String,
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        let (_, fields) = parse_model_search_config(content_leakage).unwrap();
        assert_eq!(
            fields,
            vec![
                ("title".to_string(), 'D'), // title MUST NOT inherit B from body!
                ("body".to_string(), 'B'),
            ]
        );

        // 3. Check comment stripping (both block and line comments containing #[searchable])
        let content_comments = r#"
#[autumn_web::model(table = "test_search_records")]
#[searchable(language = "english")]
pub struct SearchRecord {
    #[id]
    pub id: i64,
    // #[searchable(weight = "A")]
    // pub old_title: String,
    /*
    #[searchable(weight = "C")]
    pub commented_out: String,
    */
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        let (_, fields) = parse_model_search_config(content_comments).unwrap();
        assert_eq!(fields, vec![("body".to_string(), 'B')]);

        // 4. Check prefix collisions like searchable_fields
        let content_collision = r#"
#[autumn_web::model(table = "test_search_records")]
#[searchable_fields]
pub struct SearchRecord {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
}
"#;
        let (_, fields) = parse_model_search_config(content_collision).unwrap();
        assert_eq!(fields, vec![("title".to_string(), 'A')]);
    }

    #[test]
    fn test_singularize_ses_words() {
        assert_eq!(singularize("cases"), "case");
        assert_eq!(singularize("databases"), "database");
        assert_eq!(singularize("phases"), "phase");
        assert_eq!(singularize("uses"), "use");
        assert_eq!(singularize("statuses"), "status");
        assert_eq!(singularize("aliases"), "alias");
        assert_eq!(singularize("buses"), "bus");
    }

    #[test]
    fn test_singularize_irregular_plurals() {
        assert_eq!(singularize("people"), "person");
        assert_eq!(singularize("salespeople"), "salesperson");
        assert_eq!(singularize("children"), "child");
        assert_eq!(singularize("supermen"), "superman");
        assert_eq!(singularize("women"), "woman");
    }

    #[test]
    fn test_parse_model_search_config_helper_structs() {
        let content = r#"
pub struct HelperOne {
    pub a: i32,
}

#[autumn_web::model(table = "pages")]
#[searchable(language = "english")]
pub struct Page {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
}

pub struct HelperTwo {
    #[searchable(weight = "B")]
    pub b: String,
}
"#;
        let (lang, fields) = parse_model_search_config(content).unwrap();
        assert_eq!(lang, "english");
        assert_eq!(fields, vec![("title".to_string(), 'A')]);
    }

    #[test]
    fn test_detect_migration_shape_to_tables() {
        // Tables starting with "To" should match FTS
        match detect_migration_shape("AddSearchToTodos") {
            MigrationShape::AddSearch { table } => assert_eq!(table, "todos"),
            other => panic!("expected AddSearch, got {other:?}"),
        }
        match detect_migration_shape("AddSearchToTopics") {
            MigrationShape::AddSearch { table } => assert_eq!(table, "topics"),
            other => panic!("expected AddSearch, got {other:?}"),
        }
        // Normal column additions starting with AddSearch should fall through to AddColumns
        match detect_migration_shape("AddSearchTokenToPosts") {
            MigrationShape::AddColumns { table } => assert_eq!(table, "posts"),
            other => panic!("expected AddColumns, got {other:?}"),
        }
    }

    #[test]
    fn test_singularize_movies_and_series() {
        assert_eq!(singularize("movies"), "movie");
        assert_eq!(singularize("series"), "series");
        assert_eq!(singularize("cookies"), "cookie");
        assert_eq!(singularize("zombies"), "zombie");
    }

    #[test]
    fn test_detect_migration_shape_internal_to() {
        match detect_migration_shape("AddSearchToTopToBottoms") {
            MigrationShape::AddSearch { table } => assert_eq!(table, "top_to_bottoms"),
            other => panic!("expected AddSearch, got {other:?}"),
        }
        match detect_migration_shape("AddSearchToToDoItems") {
            MigrationShape::AddSearch { table } => assert_eq!(table, "to_do_items"),
            other => panic!("expected AddSearch, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_model_search_config_raw_identifiers() {
        let content = r#"
#[autumn_web::model(table = "items")]
pub struct Item {
    #[id]
    pub id: i64,
    #[searchable]
    pub r#type: String,
    #[searchable(weight = "B")]
    pub r#match: String,
}
"#;
        let (lang, fields) = parse_model_search_config(content).unwrap();
        assert_eq!(lang, "simple");
        assert_eq!(
            fields,
            vec![("type".to_string(), 'D'), ("match".to_string(), 'B')]
        );
    }

    #[test]
    fn test_parse_model_search_config_reverse_lookup() {
        // Helper struct before the model has a #[searchable(language = "french")] attribute.
        // We want to make sure the model struct parses its own #[searchable(language = "english")]
        // because it is closest (reverse scanning), not the earlier one.
        let content = r#"
#[searchable(language = "french")]
pub struct HelperOne {
    pub a: i32,
}

#[autumn_web::model(table = "pages")]
#[searchable(language = "english")]
pub struct Page {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
}
"#;
        let (lang, fields) = parse_model_search_config(content).unwrap();
        assert_eq!(lang, "english");
        assert_eq!(fields, vec![("title".to_string(), 'A')]);
    }

    #[test]
    fn test_strip_comments_in_string_literals() {
        let content = r#"
        let url = "https://example.com/api"; // this is a comment
        /* block comment */
        let regex = r"//[a-z]+";
        "#;
        let stripped = strip_comments(content);
        assert!(stripped.contains("https://example.com/api"));
        assert!(!stripped.contains("this is a comment"));
        assert!(!stripped.contains("block comment"));
    }

    #[test]
    fn test_parse_model_search_config_for_table_multi() {
        let content = r#"
#[autumn_web::model(table = "posts")]
#[searchable(language = "english")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
}

#[autumn_web::model(table = "comments")]
#[searchable(language = "spanish")]
pub struct Comment {
    #[id]
    pub id: i64,
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        // Verify post scanning
        let (post_lang, post_fields) =
            parse_model_search_config_for_table(content, "posts").unwrap();
        assert_eq!(post_lang, "english");
        assert_eq!(post_fields, vec![("title".to_string(), 'A')]);

        // Verify comment scanning
        let (comment_lang, comment_fields) =
            parse_model_search_config_for_table(content, "comments").unwrap();
        assert_eq!(comment_lang, "spanish");
        assert_eq!(comment_fields, vec![("body".to_string(), 'B')]);
    }

    #[test]
    fn test_strip_comments_raw_strings() {
        let content = r###"
        let r1 = r#"quote " inside raw"#;
        let r2 = r##"// not comment inside raw block"##;
        let r3 = r#"/* not block comment */"#;
        "###;
        let stripped = strip_comments(content);
        assert!(stripped.contains("r#\"quote \" inside raw\"#"));
        assert!(stripped.contains("r##\"// not comment inside raw block\"##"));
        assert!(stripped.contains("r#\"/* not block comment */\"#"));
    }

    #[test]
    fn test_parse_model_search_config_spacing_insensitivity() {
        let content_spacing = r#"
#[autumn_web::model( table  =   "posts" )]
#[searchable(language = "english")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
}
"#;
        let (lang, fields) = parse_model_search_config_for_table(content_spacing, "posts").unwrap();
        assert_eq!(lang, "english");
        assert_eq!(fields, vec![("title".to_string(), 'A')]);
    }

    #[test]
    fn test_parse_model_search_config_pascal_fallback() {
        let content = r#"
#[autumn_web::model]
#[searchable(language = "english")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
}

#[autumn_web::model]
#[searchable(language = "spanish")]
pub struct Comment {
    #[id]
    pub id: i64,
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        let (lang, fields) = parse_model_search_config_for_table(content, "comments").unwrap();
        assert_eq!(lang, "spanish");
        assert_eq!(fields, vec![("body".to_string(), 'B')]);
    }

    #[test]
    fn test_singularize_specimens_and_gentlemen() {
        assert_eq!(singularize("specimens"), "specimen");
        assert_eq!(singularize("regimens"), "regimen");
        assert_eq!(singularize("gentlemen"), "gentleman");
        assert_eq!(singularize("firemen"), "fireman");
    }

    #[test]
    fn test_singularize_ses_trailing_e() {
        assert_eq!(singularize("houses"), "house");
        assert_eq!(singularize("phrases"), "phrase");
        assert_eq!(singularize("guesses"), "guess");
        assert_eq!(singularize("lenses"), "lens");
        assert_eq!(singularize("databases"), "database");
        assert_eq!(singularize("cases"), "case");
    }

    #[test]
    fn test_parse_model_search_config_raw_string_table_attr() {
        let content_raw_table = r##"
#[autumn_web::model(table = r#"raw_posts"#)]
#[searchable(language = "english")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    pub title: String,
}
"##;
        let (lang, fields) =
            parse_model_search_config_for_table(content_raw_table, "raw_posts").unwrap();
        assert_eq!(lang, "english");
        assert_eq!(fields, vec![("title".to_string(), 'A')]);
    }

    #[test]
    fn test_parse_model_search_config_braces_in_attributes() {
        let content_braces = r#"
#[autumn_web::model(table = "posts")]
#[searchable(language = "english")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    #[validate(custom = "foo } bar")]
    pub title: String,
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        let (_, fields) = parse_model_search_config_for_table(content_braces, "posts").unwrap();
        assert_eq!(
            fields,
            vec![("title".to_string(), 'A'), ("body".to_string(), 'B')]
        );
    }

    #[test]
    fn test_parse_model_search_config_invalid_weight_fallback() {
        let content_invalid_weight = r#"
#[autumn_web::model(table = "posts")]
#[searchable(language = "english")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "Z")]
    pub title: String,
}
"#;
        let (_, fields) =
            parse_model_search_config_for_table(content_invalid_weight, "posts").unwrap();
        assert_eq!(fields, vec![("title".to_string(), 'D')]);
    }

    #[test]
    fn test_singularize_singular_table_names_ending_in_s() {
        assert_eq!(singularize("news"), "news");
        assert_eq!(singularize("status"), "status");
        assert_eq!(singularize("alias"), "alias");
        assert_eq!(singularize("bus"), "bus");
        assert_eq!(singularize("lens"), "lens");
        assert_eq!(singularize("virus"), "virus");
        assert_eq!(singularize("canvas"), "canvas");
        assert_eq!(singularize("addresses"), "address");
        assert_eq!(singularize("address"), "address");
    }

    #[test]
    fn test_parse_model_search_config_diesel_column_name() {
        let content_diesel = r##"
#[autumn_web::model(table = "posts")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    #[diesel(column_name = "headline")]
    pub title: String,
    #[diesel(column_name = r#"content_body"#)]
    #[searchable(weight = "B")]
    pub body: String,
}
"##;
        let (_, fields) = parse_model_search_config_for_table(content_diesel, "posts").unwrap();
        assert_eq!(
            fields,
            vec![
                ("headline".to_string(), 'A'),
                ("content_body".to_string(), 'B')
            ]
        );
    }

    #[test]
    fn test_strip_comments_nested_block() {
        let content = "pub /* outer /* inner */ still outer */ struct Post {}";
        let stripped = strip_comments(content);
        assert_eq!(stripped.trim(), "pub   struct Post {}");
    }

    #[test]
    fn test_strip_comments_token_boundary() {
        let content = "pub/*comment*/struct Post";
        let stripped = strip_comments(content);
        assert_eq!(stripped.trim(), "pub struct Post");
    }

    #[test]
    fn test_parse_model_search_config_strict_fallback() {
        let content_multi = r#"
#[autumn_web::model(table = "posts")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable]
    pub title: String,
}

#[autumn_web::model(table = "comments")]
pub struct Comment {
    #[id]
    pub id: i64,
    #[searchable]
    pub body: String,
}
"#;
        // Non-existent table should return None instead of falling back to the first struct (Post)
        assert!(parse_model_search_config_for_table(content_multi, "users").is_none());

        // Valid tables should still match perfectly
        let (_, posts_fields) =
            parse_model_search_config_for_table(content_multi, "posts").unwrap();
        assert_eq!(posts_fields[0].0, "title");

        let (_, comments_fields) =
            parse_model_search_config_for_table(content_multi, "comments").unwrap();
        assert_eq!(comments_fields[0].0, "body");
    }

    #[test]
    fn test_extract_diesel_column_name_identifier() {
        let content_diesel = r#"
#[autumn_web::model(table = "posts")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    #[diesel(column_name = headline)]
    pub title: String,
    #[diesel(column_name = content_body)]
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        let (_, fields) = parse_model_search_config_for_table(content_diesel, "posts").unwrap();
        assert_eq!(
            fields,
            vec![
                ("headline".to_string(), 'A'),
                ("content_body".to_string(), 'B')
            ]
        );
    }

    #[test]
    fn test_singularize_greek_origin_nouns() {
        assert_eq!(singularize("analyses"), "analysis");
        assert_eq!(singularize("crises"), "crisis");
        assert_eq!(singularize("diagnoses"), "diagnosis");
        assert_eq!(singularize("neuroses"), "neurosis");
        assert_eq!(singularize("bases"), "basis");
        assert_eq!(singularize("oases"), "oasis");

        // English-origin standard plurals ending in ases/ises
        assert_eq!(singularize("databases"), "database");
        assert_eq!(singularize("phases"), "phase");
        assert_eq!(singularize("premises"), "premise");
    }

    #[test]
    fn test_extract_diesel_column_name_raw_identifier() {
        let content_diesel = r#"
#[autumn_web::model(table = "posts")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable(weight = "A")]
    #[diesel(column_name = r#headline)]
    pub title: String,
    #[diesel(column_name = r#content_body)]
    #[searchable(weight = "B")]
    pub body: String,
}
"#;
        let (_, fields) = parse_model_search_config_for_table(content_diesel, "posts").unwrap();
        assert_eq!(
            fields,
            vec![
                ("headline".to_string(), 'A'),
                ("content_body".to_string(), 'B')
            ]
        );
    }

    #[test]
    fn test_parse_model_search_config_model_helper_collision() {
        let content_collision = r#"
#[model_helper(table = "posts")]
pub struct Helper {}

#[autumn_web::model(table = "posts")]
pub struct Post {
    #[id]
    pub id: i64,
    #[searchable]
    pub title: String,
}
"#;
        let (_, fields) = parse_model_search_config_for_table(content_collision, "posts").unwrap();
        assert_eq!(fields[0].0, "title");
    }
}
