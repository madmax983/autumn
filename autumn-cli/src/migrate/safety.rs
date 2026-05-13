//! Migration safety classification — pure SQL pattern analysis.
//!
//! Inspects the contents of `up.sql` files and classifies each SQL statement
//! into a risk tier. The result drives `autumn migrate check`'s exit code and
//! human-readable safety report printed to stderr.
//!
//! # Known limitations
//!
//! - Statement splitting uses `;` as the delimiter with awareness of
//!   `PostgreSQL` dollar-quoted blocks (`$$…$$` and `$tag$…$tag$`).
//!   Semicolons inside a dollar-quoted function body are kept intact so that
//!   a `-- autumn-safety: reviewed` marker correctly suppresses the whole
//!   statement. Semicolons inside single-quoted string literals are not
//!   handled; that pattern is essentially absent from real migration files.
//! - Line comment stripping matches `--` by position on each line. A `--`
//!   sequence inside a string literal would be incorrectly treated as a comment
//!   start. Again, this pattern is essentially absent from real migration files.
//! - Block comment stripping (`/* … */`) similarly does not handle `/*` or `*/`
//!   tokens that appear inside string literals.

use std::fmt;

/// Risk level for a migration operation, ordered from least to most risky.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Additive, backward-compatible schema change. Safe for rolling deploys.
    Safe,
    /// May acquire a table-level lock on large datasets.
    PotentiallyBlocking,
    /// Removes data or structure; old replicas may fail until they restart.
    Destructive,
    /// Cannot be easily reversed without a multi-step expand/contract cycle.
    Irreversible,
    /// Schema change is safe but requires a separate data backfill job.
    DataBackfill,
    /// Autumn cannot auto-classify this statement. Operator review required.
    ManualReview,
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Safe => write!(f, "safe"),
            Self::PotentiallyBlocking => write!(f, "potentially-blocking"),
            Self::Destructive => write!(f, "destructive"),
            Self::Irreversible => write!(f, "irreversible"),
            Self::DataBackfill => write!(f, "data-backfill"),
            Self::ManualReview => write!(f, "manual-review"),
        }
    }
}

/// A single safety finding for one SQL statement in a migration file.
#[derive(Debug, Clone)]
pub struct SafetyFinding {
    /// Short description of the risky operation (e.g. `DROP COLUMN`).
    pub operation: String,
    /// Risk classification.
    pub risk: RiskLevel,
    /// Why this is dangerous for a rolling deploy.
    pub why: &'static str,
    /// Recommended next action for the operator.
    pub next_action: &'static str,
}

/// Classify the SQL content of an `up.sql` file and return all safety findings.
///
/// Returns an empty `Vec` when the migration is fully additive and safe.
///
/// A statement annotated with `-- autumn-safety: reviewed` is skipped entirely,
/// allowing operators to acknowledge and suppress findings they have manually
/// reviewed and accepted.
pub fn classify_sql(sql: &str) -> Vec<SafetyFinding> {
    // Strip block comments at the whole-SQL level before splitting so that a
    // semicolon inside a block comment (e.g. `/* note; end */`) does not produce
    // a spurious empty statement fragment.
    let without_block_comments = strip_block_comments(sql);
    split_statements(&without_block_comments)
        .iter()
        .filter(|stmt| !has_review_suppression(stmt))
        .flat_map(|stmt| classify_statement(&normalize_statement(stmt)))
        .collect()
}

/// Returns `true` if the raw (un-normalized) statement carries an operator
/// acknowledgement marker (`-- autumn-safety: reviewed`).
///
/// The check is done on the raw text before comment-stripping so the marker
/// is not accidentally erased.
fn has_review_suppression(stmt: &str) -> bool {
    stmt.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("--") && trimmed.contains("autumn-safety: reviewed")
    })
}

/// True iff all findings are at the `Safe` risk level (or there are none).
pub fn is_safe(findings: &[SafetyFinding]) -> bool {
    findings.iter().all(|f| f.risk == RiskLevel::Safe)
}

/// True iff any finding exceeds the `Safe` risk level.
pub fn has_unsafe_findings(findings: &[SafetyFinding]) -> bool {
    findings.iter().any(|f| f.risk > RiskLevel::Safe)
}

// ── internals ────────────────────────────────────────────────────────────────

/// Split `sql` into individual statements, using `;` as the delimiter.
///
/// Dollar-quoted blocks (`$$…$$`, `$tag$…$tag$`) are kept intact so that
/// semicolons inside a function body do not produce spurious fragments.
fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut i = 0;

    while i < sql.len() {
        let rest = &sql[i..];

        // Detect a dollar-quote opening: $identifier$ (identifier may be empty → $$).
        // When found, consume everything up to and including the matching closing tag
        // so that semicolons inside the body are not treated as statement separators.
        if let Some(after_dollar) = rest.strip_prefix('$')
            && let Some(close_in_rest1) = after_dollar.find('$')
        {
            let tag_body = &after_dollar[..close_in_rest1];
            if tag_body.chars().all(|c| c.is_alphanumeric() || c == '_') {
                let tag_len = 1 + close_in_rest1 + 1; // opening $ + body + closing $
                let tag = &rest[..tag_len];
                if let Some(close_pos) = rest[tag_len..].find(tag) {
                    // Push opening tag + body + closing tag as one chunk.
                    current.push_str(&rest[..tag_len + close_pos + tag_len]);
                    i += tag_len + close_pos + tag_len;
                } else {
                    // Unclosed dollar-quote: consume to end of input.
                    current.push_str(rest);
                    i = sql.len();
                }
                continue;
            }
        }

        if rest.starts_with(';') {
            let trimmed = current.trim().to_owned();
            if !trimmed.is_empty() {
                statements.push(trimmed);
            }
            current.clear();
            i += 1;
        } else {
            let c = rest.chars().next().unwrap();
            current.push(c);
            i += c.len_utf8();
        }
    }

    let trimmed = current.trim().to_owned();
    if !trimmed.is_empty() {
        statements.push(trimmed);
    }
    statements
}

/// Split a normalized `ALTER TABLE` statement into individual subcommand strings.
///
/// Strips the `alter table <name>` prefix and splits the remaining text on
/// commas that are not enclosed in parentheses, trimming each segment.
fn alter_table_subcommands(normalized: &str) -> Vec<&str> {
    let after_prefix = normalized.strip_prefix("alter table ").unwrap_or("");
    let subcommands_start = after_prefix.find(' ').map_or(after_prefix.len(), |i| i + 1);
    let subcommands = &after_prefix[subcommands_start..];

    let mut result = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    for (i, c) in subcommands.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                result.push(subcommands[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = subcommands[start..].trim();
    if !last.is_empty() {
        result.push(last);
    }
    result
}

/// True iff `subcommand` is an ALTER TABLE subcommand that Autumn fully classifies.
///
/// A subcommand is "known" when a specific safety rule covers all risk scenarios
/// for it (including the case where it is safe and produces no finding).
fn is_known_alter_subcommand(subcommand: &str) -> bool {
    subcommand.starts_with("add column ")
        || subcommand.starts_with("drop column ")
        || subcommand.starts_with("rename column ") // RENAME COLUMN
        || subcommand.starts_with("rename to ") // RENAME TABLE (ALTER TABLE … RENAME TO …)
        || (subcommand.starts_with("alter column ") && subcommand.contains(" type "))
}

/// Remove `/* ... */` block comments from `sql`.
///
/// Handles single-line and multi-line block comments. Unclosed block comments
/// are consumed to end-of-input. Block comments inside string literals are an
/// edge case not handled here (same limitation as `--` in strings).
fn strip_block_comments(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume '*'
            loop {
                match chars.next() {
                    Some('*') if chars.peek() == Some(&'/') => {
                        chars.next(); // consume '/'
                        break;
                    }
                    None => break, // unclosed block comment
                    _ => {}
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Strip line comments, collapse whitespace, and lowercase a single statement.
fn normalize_statement(stmt: &str) -> String {
    let without_block_comments = strip_block_comments(stmt);
    without_block_comments
        .lines()
        .map(|line| line.find("--").map_or(line, |i| &line[..i]))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Returns `true` when `sql` contains an executable concurrent index operation
/// (`CREATE [UNIQUE] INDEX CONCURRENTLY` or `DROP INDEX CONCURRENTLY`).
///
/// Uses the same comment-stripping and whitespace-normalization pipeline as
/// [`classify_sql`] so that concurrent index keywords mentioned only inside a
/// SQL comment (e.g. `-- CREATE INDEX CONCURRENTLY ...`) are not counted.
pub fn contains_concurrent_index(sql: &str) -> bool {
    split_statements(sql).iter().any(|stmt| {
        let normalized = normalize_statement(stmt);
        normalized.contains("create index concurrently ")
            || normalized.contains("create unique index concurrently ")
            || normalized.starts_with("drop index concurrently ")
    })
}

/// `PostgreSQL` STABLE/IMMUTABLE function prefixes that are safe as NOT NULL column
/// defaults: Postgres evaluates them once at statement time and stores the result as
/// a constant, so the PG 11+ fast-default path applies — no table rewrite needed.
const STABLE_FN_PREFIXES: &[&str] = &["now(", "current_timestamp(", "localtimestamp("];

/// Returns `true` when `default_expr` is a volatile function call that `PostgreSQL`
/// cannot optimise via the PG 11+ fast-constant-default path.
///
/// STABLE/IMMUTABLE functions (e.g. `now()`) are evaluated once at statement time
/// and stored as a constant, so they do not require a table rewrite.  Only truly
/// VOLATILE functions (e.g. `random()`, `gen_random_uuid()`) must be flagged.
fn is_volatile_function_default(default_expr: &str) -> bool {
    default_expr.contains('(')
        && !STABLE_FN_PREFIXES
            .iter()
            .any(|p| default_expr.starts_with(p))
}

/// Apply all pattern checks to a single normalized (lowercase, single-spaced) statement.
#[allow(clippy::too_many_lines)]
fn classify_statement(normalized: &str) -> Vec<SafetyFinding> {
    if normalized.is_empty() {
        return vec![];
    }

    let mut findings = Vec::new();

    // DROP TABLE — check first; it subsumes DROP COLUMN detection
    if normalized.starts_with("drop table") {
        findings.push(SafetyFinding {
            operation: "DROP TABLE".to_owned(),
            risk: RiskLevel::Destructive,
            why: "Drops the entire table and all its data. Old replicas that reference this \
                  table will error immediately.",
            next_action: "Use expand/contract: first deploy code that stops using the table, \
                          then drop it in a subsequent release.",
        });
        return findings;
    }

    // DROP VIEW
    if normalized.starts_with("drop view") {
        findings.push(SafetyFinding {
            operation: "DROP VIEW".to_owned(),
            risk: RiskLevel::Destructive,
            why: "Drops the view. Old replicas that query this view will error immediately \
                  during a rolling deploy.",
            next_action: "Use expand/contract: first deploy code that no longer references the \
                          view, then drop it in a subsequent release.",
        });
        return findings;
    }

    // DROP COLUMN
    if normalized.contains(" drop column ") {
        findings.push(SafetyFinding {
            operation: "DROP COLUMN".to_owned(),
            risk: RiskLevel::Destructive,
            why: "Removes a column and its data. Old replicas that SELECT or INSERT this column \
                  will error until they restart.",
            next_action: "Use expand/contract: first deploy code that no longer reads or writes \
                          this column, then drop it in the next release.",
        });
    }

    // RENAME COLUMN
    if normalized.contains(" rename column ") {
        findings.push(SafetyFinding {
            operation: "RENAME COLUMN".to_owned(),
            risk: RiskLevel::Irreversible,
            why: "Renaming a column breaks queries from old replicas that still reference the \
                  old name, causing errors during a rolling deploy.",
            next_action: "Use expand/contract: add the new column, dual-write, backfill existing \
                          rows, update all code, then drop the old column.",
        });
    }

    // RENAME TABLE
    if normalized.contains("alter table")
        && normalized.contains(" rename to ")
        && !normalized.contains(" rename column ")
    {
        findings.push(SafetyFinding {
            operation: "RENAME TABLE".to_owned(),
            risk: RiskLevel::Irreversible,
            why: "Renaming a table breaks all queries from old replicas that reference the \
                  original name.",
            next_action: "Create a view under the old name while the new name rolls out, or \
                          coordinate a maintenance window.",
        });
    }

    // ALTER COLUMN TYPE
    if let Some(i) = normalized.find("alter column")
        && normalized[i..].contains(" type ")
    {
        findings.push(SafetyFinding {
            operation: "ALTER COLUMN TYPE".to_owned(),
            risk: RiskLevel::Destructive,
            why: "Changing a column's type rewrites the column data and may be incompatible \
                  with values read by old replicas or application code.",
            next_action: "Add a new column with the target type, migrate data, update code to \
                          use the new column, then drop the old one.",
        });
    }

    // ADD COLUMN NOT NULL — checked per subcommand so that a DEFAULT on one column
    // in a multi-column ALTER TABLE does not suppress the check for other columns.
    //
    // Two unsafe cases:
    //  1. No DEFAULT at all — Postgres must validate every existing row under lock.
    //  2. Volatile DEFAULT (contains a function call) — Postgres must evaluate the
    //     function per-row and cannot use the fast constant-default path (PG 11+),
    //     so the table is still rewritten under the exclusive lock.
    if normalized.starts_with("alter table") {
        for subcommand in alter_table_subcommands(normalized) {
            if subcommand.starts_with("add column ") && subcommand.contains("not null") {
                let has_default = subcommand.contains(" default ");
                // A DEFAULT is "volatile" when it is a VOLATILE function call —
                // i.e. one that Postgres must evaluate per-row, preventing the PG11+
                // fast-constant-default path.  STABLE functions (e.g. `now()`) are
                // evaluated once at statement time and do not require a table rewrite.
                let has_volatile_default = has_default
                    && subcommand.find(" default ").is_some_and(|pos| {
                        let default_expr = subcommand[pos + " default ".len()..].trim();
                        is_volatile_function_default(default_expr)
                    });

                if !has_default {
                    findings.push(SafetyFinding {
                        operation: "ADD COLUMN NOT NULL (no default)".to_owned(),
                        risk: RiskLevel::PotentiallyBlocking,
                        why: "Adding a NOT NULL column without a DEFAULT forces Postgres to \
                              validate every existing row under an exclusive lock. On a large \
                              table this may time out.",
                        next_action: "Provide a constant DEFAULT value, or add the column as \
                                      nullable first, backfill existing rows, then add the NOT \
                                      NULL constraint in a later migration.",
                    });
                    break; // one finding per statement is sufficient
                } else if has_volatile_default {
                    findings.push(SafetyFinding {
                        operation: "ADD COLUMN NOT NULL (volatile default)".to_owned(),
                        risk: RiskLevel::PotentiallyBlocking,
                        why: "A volatile function-call DEFAULT (e.g. random(), gen_random_uuid()) \
                              is evaluated per-row: Postgres cannot use the PG11+ fast-constant \
                              path and must rewrite the entire table under an exclusive lock.",
                        next_action: "Use a constant literal DEFAULT instead (e.g. DEFAULT 0, \
                                      DEFAULT ''), or add the column nullable, backfill, then \
                                      add the NOT NULL constraint in a later migration.",
                    });
                    break; // one finding per statement is sufficient
                }
            }
        }
    }

    // Unclassified ALTER TABLE subcommand — fires when any subcommand in the statement
    // is not covered by the specific rules above. Checking all subcommands individually
    // prevents a known-safe subcommand (e.g. ADD COLUMN) from hiding an unknown one
    // (e.g. DROP CONSTRAINT) in the same multi-action ALTER TABLE.
    if normalized.starts_with("alter table") {
        let subcommands = alter_table_subcommands(normalized);
        let all_known = subcommands.iter().all(|s| is_known_alter_subcommand(s));
        if !all_known {
            findings.push(SafetyFinding {
                operation: "Unclassified ALTER TABLE".to_owned(),
                risk: RiskLevel::ManualReview,
                why: "Autumn cannot automatically assess the safety of this ALTER TABLE \
                      subcommand for a rolling deploy. Some operations (e.g. DROP CONSTRAINT, \
                      ALTER COLUMN SET NOT NULL, ADD CONSTRAINT) acquire table locks or validate \
                      existing rows.",
                next_action: "Review the statement manually. If it is safe, you may suppress \
                              this finding by adding `-- autumn-safety: reviewed` above the \
                              statement.",
            });
        }
    }

    // CREATE INDEX / CREATE UNIQUE INDEX without CONCURRENTLY
    let is_create_index =
        normalized.starts_with("create index") || normalized.starts_with("create unique index");
    let is_concurrent = normalized.starts_with("create index concurrently")
        || normalized.starts_with("create unique index concurrently");
    if is_create_index && !is_concurrent {
        findings.push(SafetyFinding {
            operation: "CREATE INDEX (non-concurrent)".to_owned(),
            risk: RiskLevel::PotentiallyBlocking,
            why: "Non-concurrent index creation holds an exclusive table lock for the entire \
                  build, blocking all reads and writes.",
            next_action: "Use CREATE INDEX CONCURRENTLY instead. Note: concurrent index \
                          creation cannot run inside a transaction block.",
        });
    }

    // Data backfill — bulk DML inside a migration requires a separate job
    if normalized.starts_with("update ")
        || normalized.starts_with("insert into ")
        || normalized.starts_with("delete from ")
    {
        findings.push(SafetyFinding {
            operation: "Bulk DML (data backfill)".to_owned(),
            risk: RiskLevel::DataBackfill,
            why: "Running bulk UPDATE or INSERT inside a migration locks rows for the duration \
                  of the transaction. On large tables this can time out or block application \
                  traffic for seconds to minutes.",
            next_action: "Run the data backfill as a separate idempotent background job or \
                          one-off task (`autumn task`) after the schema migration has deployed. \
                          Add a NOT VALID constraint first if you need the constraint enforced \
                          before the backfill completes.",
        });
    }

    // CTE-prefixed bulk DML — WITH … UPDATE / DELETE / INSERT
    // A CTE starts with `with` so the plain DML checks above don't fire.
    // Check both the outer statement (`) update/delete/insert`) and CTE bodies
    // (`(update/delete/insert`) to catch data-modifying CTEs followed by SELECT.
    if normalized.starts_with("with ")
        && (normalized.contains(") update ")
            || normalized.contains(") delete ")
            || normalized.contains(") insert into ")
            || normalized.contains("(update ")
            || normalized.contains("(delete ")
            || normalized.contains("(insert into "))
    {
        findings.push(SafetyFinding {
            operation: "Bulk DML (data backfill via CTE)".to_owned(),
            risk: RiskLevel::DataBackfill,
            why: "A CTE that writes (UPDATE, DELETE, INSERT) locks rows for the duration of the \
                  transaction. On large tables this can time out or block application traffic.",
            next_action: "Run the data backfill as a separate idempotent background job or \
                          one-off task (`autumn task`) after the schema migration has deployed.",
        });
    }

    // DROP INDEX (non-concurrent) — holds an exclusive table lock
    // Use token-aware check: `concurrently` must be the SQL option immediately after
    // `drop index`, not a substring of the index name (e.g. idx_concurrently).
    if normalized.starts_with("drop index") && !normalized.starts_with("drop index concurrently ") {
        findings.push(SafetyFinding {
            operation: "DROP INDEX (non-concurrent)".to_owned(),
            risk: RiskLevel::PotentiallyBlocking,
            why: "Non-concurrent DROP INDEX acquires an AccessExclusiveLock on the table, \
                  blocking all reads and writes for the duration of the drop.",
            next_action: "Use DROP INDEX CONCURRENTLY to avoid the exclusive table lock. \
                          Add `run_in_transaction = false` to the migration's `metadata.toml`.",
        });
    }

    // Generic catch-all — DDL/DML not matched by any rule above
    let is_known_start = normalized.starts_with("drop table")
        || normalized.starts_with("drop index")
        || normalized.starts_with("alter table") // unclassified subcommands handled above
        || normalized.starts_with("create table")
        || normalized.starts_with("create index")
        || normalized.starts_with("create unique index")
        || normalized.starts_with("update ")
        || normalized.starts_with("insert into ")
        || normalized.starts_with("delete from ")
        || normalized.starts_with("comment on")
        || normalized.starts_with("create sequence")
        || normalized.starts_with("alter sequence")
        || normalized.starts_with("drop sequence")
        || normalized.starts_with("create type")
        || normalized.starts_with("alter type")
        // `drop type` is intentionally absent — falls through to ManualReview
        || normalized.starts_with("create extension")
        || normalized.starts_with("create view")
        || normalized.starts_with("drop view")
        || normalized.starts_with("select ");

    let starts_with_ddl_keyword = normalized.starts_with("create ")
        || normalized.starts_with("drop ")
        || normalized.starts_with("alter ")
        || normalized.starts_with("truncate ")
        || normalized.starts_with("grant ")
        || normalized.starts_with("revoke ");

    if starts_with_ddl_keyword && !is_known_start {
        findings.push(SafetyFinding {
            operation: "Unclassified DDL".to_owned(),
            risk: RiskLevel::ManualReview,
            why: "Autumn cannot automatically assess the safety of this statement for a rolling \
                  deploy. Operator review is required before applying this migration in \
                  production.",
            next_action: "Review the statement manually. If it is safe, you may suppress this \
                          finding by adding `-- autumn-safety: reviewed` above the statement.",
        });
    }

    findings
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── RiskLevel ordering ────────────────────────────────────────────────────

    #[test]
    fn risk_level_ordering() {
        assert!(RiskLevel::Safe < RiskLevel::PotentiallyBlocking);
        assert!(RiskLevel::PotentiallyBlocking < RiskLevel::Destructive);
        assert!(RiskLevel::Destructive < RiskLevel::Irreversible);
        assert!(RiskLevel::Irreversible < RiskLevel::DataBackfill);
        assert!(RiskLevel::DataBackfill < RiskLevel::ManualReview);
    }

    #[test]
    fn risk_level_display() {
        assert_eq!(RiskLevel::Safe.to_string(), "safe");
        assert_eq!(
            RiskLevel::PotentiallyBlocking.to_string(),
            "potentially-blocking"
        );
        assert_eq!(RiskLevel::Destructive.to_string(), "destructive");
        assert_eq!(RiskLevel::Irreversible.to_string(), "irreversible");
        assert_eq!(RiskLevel::DataBackfill.to_string(), "data-backfill");
        assert_eq!(RiskLevel::ManualReview.to_string(), "manual-review");
    }

    // ── safe migrations ───────────────────────────────────────────────────────

    #[test]
    fn empty_sql_has_no_findings() {
        assert!(classify_sql("").is_empty());
    }

    #[test]
    fn create_table_is_safe() {
        let sql =
            "CREATE TABLE posts (\n    id BIGSERIAL PRIMARY KEY,\n    title TEXT NOT NULL\n);";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "CREATE TABLE should be safe: {findings:?}"
        );
    }

    #[test]
    fn add_nullable_column_is_safe() {
        let sql = "ALTER TABLE posts ADD COLUMN subtitle TEXT NULL;";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "ADD COLUMN NULL should be safe: {findings:?}"
        );
    }

    #[test]
    fn add_not_null_column_with_default_is_safe() {
        let sql = "ALTER TABLE posts ADD COLUMN status TEXT NOT NULL DEFAULT 'draft';";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "ADD COLUMN NOT NULL DEFAULT should be safe: {findings:?}"
        );
    }

    #[test]
    fn add_not_null_column_name_containing_default_is_blocking() {
        // Column named `defaulted_at` must not be mistaken for having a DEFAULT clause.
        let sql = "ALTER TABLE posts ADD COLUMN defaulted_at TIMESTAMP NOT NULL;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "column name containing 'default' must not suppress finding"
        );
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn create_concurrent_index_is_safe() {
        let sql = "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "CREATE INDEX CONCURRENTLY should be safe: {findings:?}"
        );
    }

    #[test]
    fn create_unique_index_concurrently_is_safe() {
        let sql = "CREATE UNIQUE INDEX CONCURRENTLY idx_posts_slug ON posts (slug);";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "CREATE UNIQUE INDEX CONCURRENTLY should be safe: {findings:?}"
        );
    }

    // ── destructive patterns ──────────────────────────────────────────────────

    #[test]
    fn drop_view_is_destructive() {
        let findings = classify_sql("DROP VIEW active_posts;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
        assert_eq!(findings[0].operation, "DROP VIEW");
    }

    #[test]
    fn drop_view_cascade_is_destructive() {
        let findings = classify_sql("DROP VIEW active_posts CASCADE;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
    }

    #[test]
    fn drop_table_is_destructive() {
        let findings = classify_sql("DROP TABLE posts;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
        assert_eq!(findings[0].operation, "DROP TABLE");
    }

    #[test]
    fn drop_column_is_destructive() {
        let findings = classify_sql("ALTER TABLE posts DROP COLUMN title;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
        assert_eq!(findings[0].operation, "DROP COLUMN");
    }

    #[test]
    fn drop_column_case_insensitive() {
        let findings = classify_sql("alter table posts drop column title;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
    }

    #[test]
    fn alter_column_type_is_destructive() {
        let findings = classify_sql("ALTER TABLE posts ALTER COLUMN score TYPE BIGINT;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
        assert_eq!(findings[0].operation, "ALTER COLUMN TYPE");
    }

    // ── irreversible patterns ─────────────────────────────────────────────────

    #[test]
    fn rename_column_is_irreversible() {
        let findings = classify_sql("ALTER TABLE posts RENAME COLUMN body TO content;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Irreversible);
        assert_eq!(findings[0].operation, "RENAME COLUMN");
    }

    #[test]
    fn rename_table_is_irreversible() {
        let findings = classify_sql("ALTER TABLE posts RENAME TO articles;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Irreversible);
        assert_eq!(findings[0].operation, "RENAME TABLE");
    }

    #[test]
    fn rename_constraint_requires_manual_review() {
        // RENAME CONSTRAINT is a schema change that Autumn cannot auto-classify —
        // it must not silently pass as safe.
        let findings = classify_sql("ALTER TABLE users RENAME CONSTRAINT old_name TO new_name;");
        assert_eq!(
            findings.len(),
            1,
            "RENAME CONSTRAINT must not silently pass: {findings:?}"
        );
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
        assert_eq!(findings[0].operation, "Unclassified ALTER TABLE");
    }

    // ── potentially blocking patterns ─────────────────────────────────────────

    #[test]
    fn multi_column_add_only_flags_clause_without_default() {
        // ADD COLUMN score has DEFAULT — ADD COLUMN slug does NOT. Only slug should be flagged.
        let sql = "ALTER TABLE posts \
                   ADD COLUMN score INT NOT NULL DEFAULT 0, \
                   ADD COLUMN slug TEXT NOT NULL;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "only the column without a DEFAULT should be flagged"
        );
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn mixed_known_and_unknown_alter_table_subcommands_flagged() {
        // ADD COLUMN is safe, but DROP CONSTRAINT is unclassified — should get ManualReview.
        let sql = "ALTER TABLE posts ADD COLUMN subtitle TEXT, DROP CONSTRAINT posts_title_key;";
        let findings = classify_sql(sql);
        assert!(
            findings.iter().any(|f| f.risk == RiskLevel::ManualReview),
            "unknown subcommand must produce ManualReview: {findings:?}"
        );
    }

    #[test]
    fn add_not_null_column_without_default_is_blocking() {
        let findings = classify_sql("ALTER TABLE posts ADD COLUMN score INTEGER NOT NULL;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
        assert_eq!(findings[0].operation, "ADD COLUMN NOT NULL (no default)");
    }

    #[test]
    fn add_not_null_column_with_volatile_default_is_blocking() {
        let findings = classify_sql(
            "ALTER TABLE posts ADD COLUMN token UUID NOT NULL DEFAULT gen_random_uuid();",
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
        assert_eq!(
            findings[0].operation,
            "ADD COLUMN NOT NULL (volatile default)"
        );
    }

    #[test]
    fn add_not_null_column_with_now_default_is_safe() {
        // now() is STABLE: Postgres evaluates it once at statement time and stores the
        // constant, so the PG11+ fast-default path applies — no table rewrite needed.
        let findings = classify_sql(
            "ALTER TABLE posts ADD COLUMN created_at TIMESTAMP NOT NULL DEFAULT now();",
        );
        assert!(
            findings.is_empty(),
            "DEFAULT now() is stable and must not be flagged as volatile: {findings:?}"
        );
    }

    #[test]
    fn add_not_null_column_with_constant_default_is_safe() {
        // Constant literals use the PG11+ fast path — no table rewrite.
        let findings =
            classify_sql("ALTER TABLE posts ADD COLUMN active BOOLEAN NOT NULL DEFAULT false;");
        assert!(
            findings.is_empty(),
            "constant DEFAULT false must be safe: {findings:?}"
        );
    }

    #[test]
    fn create_non_concurrent_index_is_blocking() {
        let findings = classify_sql("CREATE INDEX idx_posts_title ON posts (title);");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
        assert_eq!(findings[0].operation, "CREATE INDEX (non-concurrent)");
    }

    #[test]
    fn create_unique_index_without_concurrently_is_blocking() {
        let findings = classify_sql("CREATE UNIQUE INDEX idx_posts_slug ON posts (slug);");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
        assert_eq!(findings[0].operation, "CREATE INDEX (non-concurrent)");
    }

    // ── multi-statement SQL ───────────────────────────────────────────────────

    #[test]
    fn multiple_safe_statements_produce_no_findings() {
        let sql = "\
            ALTER TABLE posts ADD COLUMN subtitle TEXT NULL;\n\
            CREATE INDEX CONCURRENTLY idx_posts_subtitle ON posts (subtitle);";
        let findings = classify_sql(sql);
        assert!(findings.is_empty());
    }

    #[test]
    fn mixed_safe_and_unsafe_statements_produces_findings_for_unsafe() {
        let sql = "\
            ALTER TABLE posts ADD COLUMN subtitle TEXT NULL;\n\
            ALTER TABLE posts DROP COLUMN body;";
        let findings = classify_sql(sql);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
    }

    #[test]
    fn multiple_unsafe_statements_produce_multiple_findings() {
        let sql = "\
            ALTER TABLE posts DROP COLUMN body;\n\
            CREATE INDEX idx_posts_title ON posts (title);";
        let findings = classify_sql(sql);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().any(|f| f.risk == RiskLevel::Destructive));
        assert!(
            findings
                .iter()
                .any(|f| f.risk == RiskLevel::PotentiallyBlocking)
        );
    }

    // ── line comments are ignored ─────────────────────────────────────────────

    #[test]
    fn sql_with_line_comments_is_classified_correctly() {
        let sql = "-- Removing old column\nALTER TABLE posts DROP COLUMN body;";
        let findings = classify_sql(sql);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
    }

    #[test]
    fn autumn_safety_comment_does_not_double_classify() {
        // Autumn-generated SQL includes a leading safety comment; ensure the
        // comment text itself doesn't trigger a second finding.
        let sql = "-- autumn-safety: destructive\nALTER TABLE posts DROP COLUMN body;";
        let findings = classify_sql(sql);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn autumn_safety_reviewed_suppresses_manual_review_finding() {
        // Operator acknowledges a CREATE FUNCTION is safe for their deploy.
        let sql = "-- autumn-safety: reviewed\nCREATE FUNCTION noop() RETURNS void LANGUAGE sql AS $$SELECT 1$$;";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "reviewed marker must suppress ManualReview finding: {findings:?}"
        );
    }

    #[test]
    fn autumn_safety_reviewed_suppresses_unclassified_alter_table() {
        let sql = "-- autumn-safety: reviewed\nALTER TABLE users DROP CONSTRAINT users_email_key;";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "reviewed marker must suppress finding: {findings:?}"
        );
    }

    #[test]
    fn autumn_safety_destructive_does_not_suppress() {
        // Only the `reviewed` marker suppresses; other autumn-safety values are informational.
        let sql = "-- autumn-safety: destructive\nALTER TABLE posts DROP COLUMN body;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "non-reviewed marker must not suppress findings"
        );
    }

    // ── helper predicates ─────────────────────────────────────────────────────

    #[test]
    fn is_safe_returns_true_for_empty() {
        assert!(is_safe(&[]));
    }

    #[test]
    fn is_safe_returns_false_for_unsafe_findings() {
        let f = SafetyFinding {
            operation: "DROP COLUMN".to_owned(),
            risk: RiskLevel::Destructive,
            why: "",
            next_action: "",
        };
        assert!(!is_safe(&[f]));
    }

    #[test]
    fn has_unsafe_findings_returns_false_for_empty() {
        assert!(!has_unsafe_findings(&[]));
    }

    #[test]
    fn has_unsafe_findings_returns_true_for_blocking() {
        let f = SafetyFinding {
            operation: "CREATE INDEX (non-concurrent)".to_owned(),
            risk: RiskLevel::PotentiallyBlocking,
            why: "",
            next_action: "",
        };
        assert!(has_unsafe_findings(&[f]));
    }

    // ── finding fields carry useful guidance ─────────────────────────────────

    #[test]
    fn drop_column_finding_names_the_risk_and_next_action() {
        let findings = classify_sql("ALTER TABLE posts DROP COLUMN body;");
        let f = &findings[0];
        assert!(
            !f.why.is_empty(),
            "why must explain the rolling-deploy risk"
        );
        assert!(
            !f.next_action.is_empty(),
            "next_action must tell the operator what to do"
        );
    }

    #[test]
    fn non_concurrent_index_finding_mentions_concurrently() {
        let findings = classify_sql("CREATE INDEX idx ON posts (title);");
        let f = &findings[0];
        assert!(
            f.next_action.to_lowercase().contains("concurrently"),
            "next_action should recommend CONCURRENTLY: {}",
            f.next_action
        );
    }

    // ── data backfill patterns ────────────────────────────────────────────────

    #[test]
    fn bulk_update_is_data_backfill() {
        let findings = classify_sql("UPDATE posts SET status = 'draft' WHERE status IS NULL;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::DataBackfill);
        assert_eq!(findings[0].operation, "Bulk DML (data backfill)");
    }

    #[test]
    fn insert_select_is_data_backfill() {
        let findings =
            classify_sql("INSERT INTO post_tags (post_id, tag) SELECT id, 'untagged' FROM posts;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::DataBackfill);
    }

    #[test]
    fn bulk_delete_is_data_backfill() {
        let findings = classify_sql("DELETE FROM posts WHERE created_at < '2020-01-01';");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::DataBackfill);
        assert_eq!(findings[0].operation, "Bulk DML (data backfill)");
    }

    #[test]
    fn data_backfill_finding_recommends_separate_job() {
        let findings = classify_sql("UPDATE posts SET slug = LOWER(title);");
        let f = &findings[0];
        assert!(!f.why.is_empty());
        assert!(
            f.next_action.to_lowercase().contains("background job")
                || f.next_action.to_lowercase().contains("task"),
            "next_action should recommend a separate job or task: {}",
            f.next_action
        );
    }

    #[test]
    fn cte_update_is_data_backfill() {
        let sql = "WITH batch AS (SELECT id FROM posts WHERE status IS NULL LIMIT 1000) \
                   UPDATE posts SET status = 'draft' FROM batch WHERE posts.id = batch.id;";
        let findings = classify_sql(sql);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::DataBackfill);
        assert_eq!(findings[0].operation, "Bulk DML (data backfill via CTE)");
    }

    #[test]
    fn cte_delete_is_data_backfill() {
        let sql = "WITH doomed AS (SELECT id FROM posts WHERE archived = true) DELETE FROM posts USING doomed WHERE posts.id = doomed.id;";
        let findings = classify_sql(sql);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::DataBackfill);
    }

    #[test]
    fn cte_select_only_is_safe() {
        // A read-only CTE should not produce a DataBackfill finding.
        let sql = "WITH recent AS (SELECT id FROM posts ORDER BY created_at DESC LIMIT 10) \
                   SELECT * FROM recent;";
        let findings = classify_sql(sql);
        assert!(
            findings.iter().all(|f| f.risk != RiskLevel::DataBackfill),
            "read-only CTE must not produce DataBackfill finding: {findings:?}"
        );
    }

    #[test]
    fn cte_body_write_with_outer_select_is_data_backfill() {
        // data-modifying CTE where the outer statement is SELECT, not UPDATE/DELETE.
        let sql = "WITH touched AS \
                   (UPDATE posts SET migrated = true WHERE migrated IS NULL RETURNING id) \
                   SELECT count(*) FROM touched;";
        let findings = classify_sql(sql);
        assert!(
            findings.iter().any(|f| f.risk == RiskLevel::DataBackfill),
            "data-modifying CTE with outer SELECT must still be flagged: {findings:?}"
        );
    }

    // ── manual review patterns ────────────────────────────────────────────────

    #[test]
    fn create_function_requires_manual_review() {
        let sql = "CREATE FUNCTION update_modified() RETURNS trigger AS $$ BEGIN NEW.updated_at = now(); RETURN NEW; END; $$ LANGUAGE plpgsql;";
        let findings = classify_sql(sql);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
        assert_eq!(findings[0].operation, "Unclassified DDL");
    }

    #[test]
    fn truncate_requires_manual_review() {
        let findings = classify_sql("TRUNCATE TABLE staging_data;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
    }

    #[test]
    fn drop_constraint_requires_manual_review() {
        let findings = classify_sql("ALTER TABLE users DROP CONSTRAINT users_email_key;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
        assert_eq!(findings[0].operation, "Unclassified ALTER TABLE");
    }

    #[test]
    fn alter_column_set_not_null_requires_manual_review() {
        let findings = classify_sql("ALTER TABLE users ALTER COLUMN email SET NOT NULL;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
        assert_eq!(findings[0].operation, "Unclassified ALTER TABLE");
    }

    #[test]
    fn known_ddl_does_not_trigger_manual_review() {
        // CREATE TABLE is safe — must not get a ManualReview finding on top
        let findings = classify_sql("CREATE TABLE comments (id BIGSERIAL PRIMARY KEY);");
        assert!(
            findings.iter().all(|f| f.risk != RiskLevel::ManualReview),
            "known DDL should not also produce ManualReview: {findings:?}"
        );
    }

    #[test]
    fn add_column_does_not_trigger_unclassified_alter_table() {
        // A safe ADD COLUMN must not also generate a ManualReview finding.
        let findings = classify_sql("ALTER TABLE posts ADD COLUMN subtitle TEXT NULL;");
        assert!(
            findings.iter().all(|f| f.risk != RiskLevel::ManualReview),
            "safe ADD COLUMN should not produce ManualReview: {findings:?}"
        );
    }

    // ── contains_concurrent_index ─────────────────────────────────────────────

    #[test]
    fn contains_concurrent_index_true_for_executable_statement() {
        assert!(contains_concurrent_index(
            "CREATE INDEX CONCURRENTLY idx ON posts (title);"
        ));
        assert!(contains_concurrent_index(
            "CREATE UNIQUE INDEX CONCURRENTLY idx ON posts (slug);"
        ));
    }

    #[test]
    fn contains_concurrent_index_false_for_non_concurrent() {
        assert!(!contains_concurrent_index(
            "CREATE INDEX idx ON posts (title);"
        ));
    }

    #[test]
    fn contains_concurrent_index_false_for_comment_only_mention() {
        let sql = "-- TODO: use CREATE INDEX CONCURRENTLY later\n\
                   CREATE INDEX idx ON posts (title);";
        assert!(
            !contains_concurrent_index(sql),
            "a CONCURRENTLY reference only in a comment must return false"
        );
    }

    #[test]
    fn contains_concurrent_index_true_for_multiline_statement() {
        let sql = "CREATE INDEX\n  CONCURRENTLY idx_posts_title ON posts (title);";
        assert!(
            contains_concurrent_index(sql),
            "multi-line CONCURRENTLY statement must be detected"
        );
    }

    #[test]
    fn contains_concurrent_index_true_for_drop_index_concurrently() {
        assert!(
            contains_concurrent_index("DROP INDEX CONCURRENTLY idx_posts_title;"),
            "DROP INDEX CONCURRENTLY must be detected"
        );
    }

    // ── block comment stripping ───────────────────────────────────────────────

    #[test]
    fn block_comment_before_drop_table_is_still_classified() {
        let sql = "/* cleanup old table */ DROP TABLE posts;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "DROP TABLE must be found after block comment"
        );
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
    }

    #[test]
    fn block_comment_before_create_index_is_still_classified() {
        let sql = "/* needs index */ CREATE INDEX idx ON posts (title);";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "CREATE INDEX must be found after block comment"
        );
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
    }

    #[test]
    fn multiline_block_comment_is_stripped() {
        let sql = "/*\n * Remove legacy column\n */\nALTER TABLE posts DROP COLUMN body;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "DROP COLUMN must be found after multi-line block comment"
        );
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
    }

    #[test]
    fn block_comment_only_mention_of_keyword_is_not_classified() {
        // Only mentions DROP TABLE inside a block comment; actual statement is safe.
        let sql = "/* was: DROP TABLE posts; */ ALTER TABLE posts ADD COLUMN active BOOL;";
        let findings = classify_sql(sql);
        assert!(
            findings.iter().all(|f| f.risk != RiskLevel::Destructive),
            "Destructive keyword inside block comment must not produce a Destructive finding"
        );
    }

    #[test]
    fn block_comment_with_semicolon_does_not_hide_following_statement() {
        // The semicolon inside the block comment must not split the statement early.
        let sql = "/* cleanup; safe to drop */ DROP TABLE posts;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "DROP TABLE after a block comment containing ';' must still be classified"
        );
        assert_eq!(findings[0].risk, RiskLevel::Destructive);
    }

    // ── dollar-quoted function bodies ─────────────────────────────────────────

    #[test]
    fn dollar_quoted_function_with_semicolons_is_one_statement() {
        // The semicolons inside $$ ... $$ must not produce extra statement fragments.
        let sql = "CREATE FUNCTION bump() RETURNS void AS $$ BEGIN \
                   UPDATE posts SET hits = hits + 1; RETURN; END; $$ LANGUAGE plpgsql;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "dollar-quoted body with semicolons must produce exactly one finding: {findings:?}"
        );
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
    }

    #[test]
    fn autumn_safety_reviewed_suppresses_function_with_dml_in_body() {
        // Without dollar-quote-aware splitting the DML fragment would escape suppression.
        let sql = "-- autumn-safety: reviewed\n\
                   CREATE FUNCTION migrate_posts() RETURNS void AS $$\n\
                   BEGIN\n  UPDATE posts SET migrated = true;\n  RETURN;\nEND;\n\
                   $$ LANGUAGE plpgsql;";
        let findings = classify_sql(sql);
        assert!(
            findings.is_empty(),
            "reviewed marker must suppress a dollar-quoted function containing DML: {findings:?}"
        );
    }

    #[test]
    fn tagged_dollar_quote_with_semicolons_is_one_statement() {
        let sql = "CREATE FUNCTION foo() RETURNS void AS $func$ \
                   BEGIN UPDATE posts SET x = 1; END; $func$ LANGUAGE plpgsql;";
        let findings = classify_sql(sql);
        assert_eq!(
            findings.len(),
            1,
            "tagged dollar-quote body with semicolons must not split: {findings:?}"
        );
    }

    // ── DROP INDEX ────────────────────────────────────────────────────────────

    #[test]
    fn drop_index_non_concurrent_is_potentially_blocking() {
        let findings = classify_sql("DROP INDEX idx_posts_title;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
        assert_eq!(findings[0].operation, "DROP INDEX (non-concurrent)");
    }

    #[test]
    fn drop_index_with_concurrently_in_name_is_still_blocking() {
        // "concurrently" appears in the index name, not as the SQL token.
        let findings = classify_sql("DROP INDEX idx_concurrently;");
        assert_eq!(
            findings.len(),
            1,
            "index named idx_concurrently must still be flagged as non-concurrent: {findings:?}"
        );
        assert_eq!(findings[0].operation, "DROP INDEX (non-concurrent)");
    }

    #[test]
    fn drop_index_concurrently_is_safe_from_classifier() {
        // CONCURRENTLY avoids the table lock; the opt-out check in migrate.rs handles
        // the metadata.toml requirement separately.
        let findings = classify_sql("DROP INDEX CONCURRENTLY idx_posts_title;");
        assert!(
            findings
                .iter()
                .all(|f| f.risk != RiskLevel::PotentiallyBlocking
                    || f.operation.contains("CONCURRENTLY")),
            "DROP INDEX CONCURRENTLY must not produce a non-concurrent finding"
        );
    }

    // ── DROP TYPE ─────────────────────────────────────────────────────────────

    #[test]
    fn drop_type_requires_manual_review() {
        let findings = classify_sql("DROP TYPE status;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
    }

    #[test]
    fn drop_type_cascade_requires_manual_review() {
        let findings = classify_sql("DROP TYPE status CASCADE;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::ManualReview);
    }
}
