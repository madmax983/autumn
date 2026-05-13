//! Migration safety classification — pure SQL pattern analysis.
//!
//! Inspects the contents of `up.sql` files and classifies each SQL statement
//! into a risk tier. The result drives `autumn migrate check`'s exit code and
//! human-readable safety report printed to stderr.

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
/// TODO(red): stub — always returns empty; implement in green phase
pub fn classify_sql(_sql: &str) -> Vec<SafetyFinding> {
    vec![]
}

/// True iff there are no findings above `Safe` risk level.
pub fn is_safe(findings: &[SafetyFinding]) -> bool {
    findings.iter().all(|f| f.risk == RiskLevel::Safe)
}

/// True iff any finding exceeds the `Safe` risk level.
pub fn has_unsafe_findings(findings: &[SafetyFinding]) -> bool {
    findings.iter().any(|f| f.risk > RiskLevel::Safe)
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
        assert_eq!(RiskLevel::PotentiallyBlocking.to_string(), "potentially-blocking");
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
        let sql = "CREATE TABLE posts (\n    id BIGSERIAL PRIMARY KEY,\n    title TEXT NOT NULL\n);";
        let findings = classify_sql(sql);
        assert!(findings.is_empty(), "CREATE TABLE should be safe: {findings:?}");
    }

    #[test]
    fn add_nullable_column_is_safe() {
        let sql = "ALTER TABLE posts ADD COLUMN subtitle TEXT NULL;";
        let findings = classify_sql(sql);
        assert!(findings.is_empty(), "ADD COLUMN NULL should be safe: {findings:?}");
    }

    #[test]
    fn add_not_null_column_with_default_is_safe() {
        let sql = "ALTER TABLE posts ADD COLUMN status TEXT NOT NULL DEFAULT 'draft';";
        let findings = classify_sql(sql);
        assert!(findings.is_empty(), "ADD COLUMN NOT NULL DEFAULT should be safe: {findings:?}");
    }

    #[test]
    fn create_concurrent_index_is_safe() {
        let sql = "CREATE INDEX CONCURRENTLY idx_posts_title ON posts (title);";
        let findings = classify_sql(sql);
        assert!(findings.is_empty(), "CREATE INDEX CONCURRENTLY should be safe: {findings:?}");
    }

    // ── destructive patterns ──────────────────────────────────────────────────

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

    // ── potentially blocking patterns ─────────────────────────────────────────

    #[test]
    fn add_not_null_column_without_default_is_blocking() {
        let findings = classify_sql("ALTER TABLE posts ADD COLUMN score INTEGER NOT NULL;");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].risk, RiskLevel::PotentiallyBlocking);
        assert_eq!(findings[0].operation, "ADD COLUMN NOT NULL (no default)");
    }

    #[test]
    fn create_non_concurrent_index_is_blocking() {
        let findings = classify_sql("CREATE INDEX idx_posts_title ON posts (title);");
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
        assert!(findings.iter().any(|f| f.risk == RiskLevel::PotentiallyBlocking));
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
        assert!(!f.why.is_empty(), "why must explain the rolling-deploy risk");
        assert!(!f.next_action.is_empty(), "next_action must tell the operator what to do");
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
}
