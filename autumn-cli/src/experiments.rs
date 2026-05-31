//! `autumn experiments` — inspect and manage A/B experiments at runtime.
//!
//! All commands connect directly to the configured Postgres database.
//! The database URL is resolved from `autumn.toml`, profile overrides, or
//! the `AUTUMN_DATABASE__PRIMARY_URL` / `AUTUMN_DATABASE__URL` / `DATABASE_URL`
//! environment variables.
//!
//! # Commands
//!
//! ```text
//! autumn experiments list                                  # list all experiments
//! autumn experiments status <name>                         # show experiment details
//! autumn experiments set-weights <name> <v=w,v=w,...>      # update variant weights
//! autumn experiments conclude <name> <winner>              # pin winner, stop assignments
//! autumn experiments override <name> <actor_id> <variant>  # pin actor to variant (QA/staff)
//! ```

use std::process::Command;

// ── Options ───────────────────────────────────────────────────────────────────

/// Options for `autumn experiments list`.
pub struct ListOptions;

/// Options for `autumn experiments status <name>`.
pub struct StatusOptions {
    pub name: String,
}

/// Options for `autumn experiments set-weights <name> <variants>`.
pub struct SetWeightsOptions {
    pub name: String,
    /// Comma-separated `variant=weight` pairs, e.g. `"control=50,treatment=50"`.
    pub weights: String,
    pub actor: Option<String>,
}

/// Options for `autumn experiments conclude <name> <winner>`.
pub struct ConcludeOptions {
    pub name: String,
    pub winner: String,
    pub actor: Option<String>,
}

/// Options for `autumn experiments override <name> <actor_id> <variant>`.
pub struct OverrideOptions {
    pub name: String,
    pub actor_id: String,
    pub variant: String,
    pub actor: Option<String>,
}

// ── SQL helpers ──────────────────────────────────────────────────────────────

const LIST_SQL: &str = "SELECT name, state, \
    (SELECT string_agg(v->>'name' || '=' || v->>'weight', ', ' ORDER BY v->>'name') \
        FROM jsonb_array_elements(variants::jsonb) v) AS variants, \
    winner, updated_at \
    FROM autumn_experiments ORDER BY name;";

const STATUS_SQL: &str = "SELECT name, description, state, variants, winner, \
    exclusion_group, updated_at \
    FROM autumn_experiments WHERE name = :'name';";

const SET_WEIGHTS_SQL: &str = "BEGIN; \
SELECT 1/(SELECT COUNT(*)::int FROM autumn_experiments WHERE name = :'name') AS exists_check; \
UPDATE autumn_experiments SET variants = :'variants'::jsonb, updated_at = NOW() \
    WHERE name = :'name'; \
INSERT INTO autumn_experiment_changes (experiment, mutation, actor) \
    VALUES (:'name', 'set_weights=' || :'variants', :'actor'); \
COMMIT;";

const CONCLUDE_SQL: &str = "BEGIN; \
SELECT 1/(SELECT COUNT(*)::int FROM autumn_experiments WHERE name = :'name') AS exists_check; \
UPDATE autumn_experiments SET state = 'concluded', winner = :'winner', updated_at = NOW() \
    WHERE name = :'name'; \
INSERT INTO autumn_experiment_changes (experiment, mutation, actor) \
    VALUES (:'name', 'concluded=' || :'winner', :'actor'); \
COMMIT;";

const OVERRIDE_SQL: &str = "BEGIN; \
INSERT INTO autumn_experiment_overrides (experiment, actor, variant) \
    VALUES (:'name', :'actor_id', :'variant') \
    ON CONFLICT (experiment, actor) DO UPDATE SET variant = :'variant'; \
INSERT INTO autumn_experiment_changes (experiment, mutation, actor) \
    VALUES (:'name', 'override=' || :'actor_id' || ':' || :'variant', :'actor'); \
COMMIT;";

// ── Public runners ────────────────────────────────────────────────────────────

/// Run `autumn experiments list`.
pub fn run_list(_opts: &ListOptions) {
    let db_url = resolve_database_url();
    let mut cmd = psql_command(&db_url);
    cmd.arg("--command").arg("\\pset footer off");
    cmd.arg("--command").arg(LIST_SQL);
    exec(cmd, "experiments list");
}

/// Run `autumn experiments status <name>`.
pub fn run_status(opts: &StatusOptions) {
    let db_url = resolve_database_url();
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("name={}", opts.name));
    cmd.arg("--command").arg("\\pset footer off");
    cmd.arg("--command").arg(STATUS_SQL);
    exec(cmd, "experiments status");
}

/// Run `autumn experiments set-weights <name> <weights>`.
pub fn run_set_weights(opts: &SetWeightsOptions) {
    let db_url = resolve_database_url();
    let actor = opts.actor.as_deref().unwrap_or("cli");
    // Parse "control=50,treatment=50" into JSON array of variant objects.
    let variants_json = parse_weights_to_json(&opts.weights).unwrap_or_else(|e| {
        eprintln!("autumn experiments set-weights: {e}");
        std::process::exit(1);
    });
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("name={}", opts.name));
    cmd.arg("--variable").arg(format!("variants={variants_json}"));
    cmd.arg("--variable").arg(format!("actor={actor}"));
    cmd.arg("--command").arg(SET_WEIGHTS_SQL);
    exec(cmd, "experiments set-weights");
    println!("✓ Experiment '{}' weights updated to {}.", opts.name, opts.weights);
}

/// Run `autumn experiments conclude <name> <winner>`.
pub fn run_conclude(opts: &ConcludeOptions) {
    let db_url = resolve_database_url();
    let actor = opts.actor.as_deref().unwrap_or("cli");
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("name={}", opts.name));
    cmd.arg("--variable").arg(format!("winner={}", opts.winner));
    cmd.arg("--variable").arg(format!("actor={actor}"));
    cmd.arg("--command").arg(CONCLUDE_SQL);
    exec(cmd, "experiments conclude");
    println!(
        "✓ Experiment '{}' concluded with winner '{}'.",
        opts.name, opts.winner
    );
}

/// Run `autumn experiments override <name> <actor_id> <variant>`.
pub fn run_override(opts: &OverrideOptions) {
    let db_url = resolve_database_url();
    let actor = opts.actor.as_deref().unwrap_or("cli");
    let mut cmd = psql_command(&db_url);
    cmd.arg("--variable").arg(format!("name={}", opts.name));
    cmd.arg("--variable").arg(format!("actor_id={}", opts.actor_id));
    cmd.arg("--variable").arg(format!("variant={}", opts.variant));
    cmd.arg("--variable").arg(format!("actor={actor}"));
    cmd.arg("--command").arg(OVERRIDE_SQL);
    exec(cmd, "experiments override");
    println!(
        "✓ Actor '{}' pinned to variant '{}' in experiment '{}'.",
        opts.actor_id, opts.variant, opts.name
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert `"control=50,treatment=50"` to `[{"name":"control","weight":50},...]`.
/// Returns an error message if any pair is malformed.
fn parse_weights_to_json(weights: &str) -> Result<String, String> {
    let mut parts: Vec<serde_json::Value> = Vec::new();
    for raw in weights.split(',') {
        let pair = raw.trim();
        let mut it = pair.splitn(2, '=');
        let name = it.next().unwrap_or("").trim();
        if name.is_empty() {
            return Err(format!("malformed weight spec {pair:?}: variant name is empty"));
        }
        let weight_str = it
            .next()
            .ok_or_else(|| format!("malformed weight spec {pair:?}: missing '=<weight>'"))?;
        let weight: u32 = weight_str.trim().parse().map_err(|_| {
            format!("malformed weight spec {pair:?}: weight must be a non-negative integer")
        })?;
        parts.push(serde_json::json!({"name": name, "weight": weight}));
    }
    Ok(serde_json::to_string(&parts).unwrap_or_else(|_| "[]".to_owned()))
}

fn resolve_database_url() -> String {
    crate::config::resolve_database_url()
}

fn psql_command(db_url: &str) -> Command {
    let mut cmd = Command::new("psql");
    cmd.arg(db_url);
    cmd.arg("--no-psqlrc");
    cmd.arg("--set=ON_ERROR_STOP=on");
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
        assert!(LIST_SQL.contains("autumn_experiments"));
        assert!(STATUS_SQL.contains("autumn_experiments"));
        assert!(SET_WEIGHTS_SQL.contains("autumn_experiments"));
        assert!(SET_WEIGHTS_SQL.contains("autumn_experiment_changes"));
        assert!(CONCLUDE_SQL.contains("autumn_experiments"));
        assert!(CONCLUDE_SQL.contains("autumn_experiment_changes"));
        assert!(OVERRIDE_SQL.contains("autumn_experiment_overrides"));
        assert!(OVERRIDE_SQL.contains("autumn_experiment_changes"));
    }

    #[test]
    fn sql_mutations_use_transactions() {
        for sql in [SET_WEIGHTS_SQL, CONCLUDE_SQL, OVERRIDE_SQL] {
            assert!(sql.starts_with("BEGIN;"), "mutation SQL must use BEGIN: {sql}");
            assert!(sql.contains("COMMIT;"), "mutation SQL must use COMMIT: {sql}");
        }
    }

    #[test]
    fn conclude_sql_sets_winner_and_state() {
        assert!(CONCLUDE_SQL.contains("state = 'concluded'"));
        assert!(CONCLUDE_SQL.contains("winner = :'winner'"));
    }

    #[test]
    fn override_sql_uses_upsert() {
        assert!(
            OVERRIDE_SQL.contains("ON CONFLICT"),
            "override SQL must use INSERT ... ON CONFLICT"
        );
    }

    #[test]
    fn parse_weights_to_json_produces_valid_json() {
        let json = parse_weights_to_json("control=50,treatment=50").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "control");
        assert_eq!(arr[0]["weight"], 50);
        assert_eq!(arr[1]["name"], "treatment");
        assert_eq!(arr[1]["weight"], 50);
    }

    #[test]
    fn parse_weights_handles_three_variants() {
        let json = parse_weights_to_json("control=33,treatment_a=33,treatment_b=34").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        assert_eq!(v.as_array().unwrap().len(), 3);
    }

    #[test]
    fn parse_weights_handles_whitespace() {
        let json = parse_weights_to_json(" control = 50 , treatment = 50 ").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0]["name"], "control");
    }

    #[test]
    fn parse_weights_errors_on_missing_equals() {
        let err = parse_weights_to_json("control50").unwrap_err();
        assert!(err.contains("malformed"), "expected malformed error, got: {err}");
    }

    #[test]
    fn parse_weights_errors_on_non_integer_weight() {
        let err = parse_weights_to_json("control=abc").unwrap_err();
        assert!(err.contains("integer"), "expected integer error, got: {err}");
    }
}
