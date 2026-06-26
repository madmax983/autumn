//! CI guard: schema-key snapshot integrity.
//!
//! Ensures that any config key removed from the compiled schema is either:
//!   (a) still present in the schema (no drift), or
//!   (b) registered in `DEPRECATED_CONFIG_KEYS` (proper deprecation ramp).
//!
//! # Regenerating the snapshot
//!
//! Run with `UPDATE_SCHEMA_SNAPSHOT=1` to write the current schema to the
//! snapshot file:
//!
//! ```
//! UPDATE_SCHEMA_SNAPSHOT=1 cargo test -p autumn-web schema_keys_snapshot_guard
//! ```
//!
//! This test is pinned to the features compiled in the workspace default feature
//! set.  Run `cargo test --all-features` when the feature set changes.

use autumn_web::config::{AutumnConfig, deprecated_config_keys};
use std::collections::BTreeSet;

const SNAPSHOT_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/schema_keys.snapshot",
);

fn load_snapshot() -> BTreeSet<String> {
    let content = std::fs::read_to_string(SNAPSHOT_PATH)
        .expect("schema_keys.snapshot missing; run with UPDATE_SCHEMA_SNAPSHOT=1 to generate");
    content
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn write_snapshot(leaves: &BTreeSet<String>) {
    let content: String = leaves.iter().fold(String::new(), |mut s, l| { s.push_str(l); s.push('\n'); s });
    std::fs::write(SNAPSHOT_PATH, content).expect("failed to write schema_keys.snapshot");
}

// ── Test 15: CI guard ─────────────────────────────────────────────────────────

#[test]
fn schema_keys_snapshot_guard() {
    let current = AutumnConfig::schema_leaf_paths();

    if std::env::var("UPDATE_SCHEMA_SNAPSHOT").is_ok() {
        write_snapshot(&current);
        println!("schema_keys.snapshot updated with {} keys", current.len());
        return;
    }

    let snapshot = load_snapshot();
    let registry: BTreeSet<&str> = deprecated_config_keys().iter().map(|d| d.path).collect();

    // Keys in snapshot but absent from current schema without a registry entry.
    let removed_without_deprecation: Vec<&str> = snapshot
        .iter()
        .filter(|k| !current.contains(k.as_str()))
        .filter(|k| !registry.contains(k.as_str()))
        .map(String::as_str)
        .collect();

    assert!(
        removed_without_deprecation.is_empty(),
        "Schema keys were removed without a corresponding DEPRECATED_CONFIG_KEYS entry.\n\
         Register a deprecation for each key below, or regenerate the snapshot if the \
         removal is intentional:\n{:?}\n\n\
         Regenerate: UPDATE_SCHEMA_SNAPSHOT=1 cargo test -p autumn-web schema_keys_snapshot_guard",
        removed_without_deprecation,
    );

    // Keys in current schema but absent from snapshot → prompt to regenerate.
    let added_without_snapshot: Vec<&str> = current
        .iter()
        .filter(|k| !snapshot.contains(k.as_str()))
        .map(String::as_str)
        .collect();

    assert!(
        added_without_snapshot.is_empty(),
        "New schema keys are not in the snapshot; regenerate it:\n{:?}\n\n\
         Regenerate: UPDATE_SCHEMA_SNAPSHOT=1 cargo test -p autumn-web schema_keys_snapshot_guard",
        added_without_snapshot,
    );
}

// ── Test 16: unit — schema_leaf_paths content ─────────────────────────────────

#[test]
fn schema_leaf_paths_contains_known_paths() {
    let leaves = AutumnConfig::schema_leaf_paths();
    assert!(
        leaves.contains("server.port"),
        "server.port must be a schema leaf"
    );
    assert!(
        leaves.contains("security.rate_limit.trusted_proxies") || leaves.contains("security"),
        "security config must appear in schema leaves"
    );
}
