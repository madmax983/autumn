//! `autumn flags` — inspect and toggle feature flags at runtime.
//!
//! All commands connect directly to the configured Postgres database.
//! The database URL is resolved from `autumn.toml`, profile overrides, or
//! the `AUTUMN_DATABASE__PRIMARY_URL` / `AUTUMN_DATABASE__URL` / `DATABASE_URL`
//! environment variables.
//!
//! # Commands
//!
//! ```text
//! autumn flags list                       # list all flags with their current state
//! autumn flags enable <key>               # globally enable a flag (all actors)
//! autumn flags disable <key>              # globally disable a flag
//! autumn flags set-rollout <key> <pct>    # enable for pct% of actors (0–100)
//! autumn flags allow <key> <actor_id>     # add actor_id to the explicit allowlist
//! ```

use std::process::Command;

// ── Options ───────────────────────────────────────────────────────────────────

/// Options for `autumn flags list`.
pub struct ListOptions;

/// Options for `autumn flags enable <key>`.
pub struct EnableOptions {
    pub key: String,
    pub actor: Option<String>,
}

/// Options for `autumn flags disable <key>`.
pub struct DisableOptions {
    pub key: String,
    pub actor: Option<String>,
}

/// Options for `autumn flags set-rollout <key> <pct>`.
pub struct SetRolloutOptions {
    pub key: String,
    pub pct: u8,
    pub actor: Option<String>,
}

/// Options for `autumn flags allow <key> <actor_id>`.
pub struct AllowOptions {
    pub key: String,
    pub actor_id: String,
    pub actor: Option<String>,
}

// ── SQL helpers ──────────────────────────────────────────────────────────────

const LIST_SQL: &str = "\\pset footer off \
    SELECT key, \
           CASE WHEN enabled THEN 'YES' ELSE 'no' END AS enabled, \
           rollout_pct || '%' AS rollout, \
           actor_allowlist, \
           group_allowlist, \
           updated_at \
    FROM autumn_feature_flags ORDER BY key;";

const ENABLE_SQL: &str = "INSERT INTO autumn_feature_flags (key, enabled) \
    VALUES (:'key', TRUE) \
    ON CONFLICT (key) DO UPDATE SET enabled = TRUE, updated_at = NOW(); \
INSERT INTO feature_flag_changes (key, mutation, actor) \
    VALUES (:'key', 'enabled', :'actor');";

const DISABLE_SQL: &str = "INSERT INTO autumn_feature_flags (key, enabled) \
    VALUES (:'key', FALSE) \
    ON CONFLICT (key) DO UPDATE SET enabled = FALSE, updated_at = NOW(); \
INSERT INTO feature_flag_changes (key, mutation, actor) \
    VALUES (:'key', 'disabled', :'actor');";

const SET_ROLLOUT_SQL: &str = "INSERT INTO autumn_feature_flags (key, rollout_pct) \
    VALUES (:'key', :'pct'::smallint) \
    ON CONFLICT (key) DO UPDATE SET rollout_pct = :'pct'::smallint, updated_at = NOW(); \
INSERT INTO feature_flag_changes (key, mutation, actor) \
    VALUES (:'key', 'rollout=' || :'pct', :'actor');";

const ALLOW_SQL: &str = "INSERT INTO autumn_feature_flags (key) \
    VALUES (:'key') ON CONFLICT (key) DO NOTHING; \
UPDATE autumn_feature_flags \
    SET actor_allowlist = ( \
        SELECT json_agg(DISTINCT elem)::text \
        FROM ( \
            SELECT jsonb_array_elements_text(actor_allowlist::jsonb) AS elem \
            UNION SELECT :'actor_id' \
        ) t \
    ), updated_at = NOW() \
    WHERE key = :'key'; \
INSERT INTO feature_flag_changes (key, mutation, actor) \
    VALUES (:'key', 'allowed_actor=' || :'actor_id', :'actor');";

// ── Public runners ────────────────────────────────────────────────────────────

/// Run `autumn flags list`.
pub fn run_list(_opts: &ListOptions) {
    let db_url = resolve_database_url();
    let mut cmd = psql_command(&db_url);
    cmd.arg("--command").arg(LIST_SQL);
    exec(cmd, "flags list");
}

/// Run `autumn flags enable <key>`.
pub fn run_enable(opts: &EnableOptions) {
    let db_url = resolve_database_url();
    let actor = opts.actor.as_deref().unwrap_or("cli");
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("key={}", opts.key));
    cmd.arg("--variable").arg(format!("actor={actor}"));
    cmd.arg("--command").arg(ENABLE_SQL);
    exec(cmd, "flags enable");
    println!("✓ Flag '{}' enabled globally.", opts.key);
}

/// Run `autumn flags disable <key>`.
pub fn run_disable(opts: &DisableOptions) {
    let db_url = resolve_database_url();
    let actor = opts.actor.as_deref().unwrap_or("cli");
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("key={}", opts.key));
    cmd.arg("--variable").arg(format!("actor={actor}"));
    cmd.arg("--command").arg(DISABLE_SQL);
    exec(cmd, "flags disable");
    println!("✓ Flag '{}' disabled globally.", opts.key);
}

/// Run `autumn flags set-rollout <key> <pct>`.
pub fn run_set_rollout(opts: &SetRolloutOptions) {
    let db_url = resolve_database_url();
    let actor = opts.actor.as_deref().unwrap_or("cli");
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("key={}", opts.key));
    cmd.arg("--variable").arg(format!("pct={}", opts.pct));
    cmd.arg("--variable").arg(format!("actor={actor}"));
    cmd.arg("--command").arg(SET_ROLLOUT_SQL);
    exec(cmd, "flags set-rollout");
    println!("✓ Flag '{}' rollout set to {}%.", opts.key, opts.pct);
}

/// Run `autumn flags allow <key> <actor_id>`.
pub fn run_allow(opts: &AllowOptions) {
    let db_url = resolve_database_url();
    let actor = opts.actor.as_deref().unwrap_or("cli");
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("key={}", opts.key));
    cmd.arg("--variable")
        .arg(format!("actor_id={}", opts.actor_id));
    cmd.arg("--variable").arg(format!("actor={actor}"));
    cmd.arg("--command").arg(ALLOW_SQL);
    exec(cmd, "flags allow");
    println!(
        "✓ Actor '{}' added to allowlist for flag '{}'.",
        opts.actor_id, opts.key
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn resolve_database_url() -> String {
    crate::config::resolve_database_url()
}

fn psql_command(db_url: &str) -> Command {
    let mut cmd = Command::new("psql");
    cmd.arg(db_url);
    cmd.arg("--no-psqlrc");
    cmd
}

fn exec(mut cmd: Command, label: &str) {
    let status = cmd.status().unwrap_or_else(|e| {
        eprintln!("autumn {label}: failed to run psql: {e}");
        std::process::exit(1);
    });
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_queries_reference_correct_tables() {
        assert!(LIST_SQL.contains("autumn_feature_flags"));
        assert!(ENABLE_SQL.contains("autumn_feature_flags"));
        assert!(ENABLE_SQL.contains("feature_flag_changes"));
        assert!(DISABLE_SQL.contains("autumn_feature_flags"));
        assert!(DISABLE_SQL.contains("feature_flag_changes"));
        assert!(SET_ROLLOUT_SQL.contains("autumn_feature_flags"));
        assert!(SET_ROLLOUT_SQL.contains("feature_flag_changes"));
        assert!(ALLOW_SQL.contains("autumn_feature_flags"));
        assert!(ALLOW_SQL.contains("feature_flag_changes"));
    }

    #[test]
    fn sql_queries_contain_notify_columns() {
        // Every mutation inserts into feature_flag_changes which triggers
        // NOTIFY via the DB trigger.
        for sql in [ENABLE_SQL, DISABLE_SQL, SET_ROLLOUT_SQL, ALLOW_SQL] {
            assert!(
                sql.contains("feature_flag_changes"),
                "mutation SQL must record into feature_flag_changes: {sql}"
            );
        }
    }

    #[test]
    fn enable_sql_uses_upsert() {
        assert!(
            ENABLE_SQL.contains("ON CONFLICT"),
            "enable SQL must use INSERT ... ON CONFLICT to create the flag if absent"
        );
    }

    #[test]
    fn disable_sql_uses_upsert() {
        assert!(
            DISABLE_SQL.contains("ON CONFLICT"),
            "disable SQL must use INSERT ... ON CONFLICT"
        );
    }
}
