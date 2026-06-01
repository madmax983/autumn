//! Compile-level proof that a full versioned `#[repository]` over a model with
//! encrypted columns expands and type-checks (insert/update/bulk/upsert + version
//! history composition). No sync Diesel imports here (would clash with the
//! generated async query code).

#![cfg(feature = "db")]

use autumn_web::model;
use autumn_web::repository;

diesel::table! {
    vault_entries (id) {
        id -> Int8,
        label -> Text,
        secret -> Text,
        lookup_key -> Text,
    }
}

#[model(table = "vault_entries")]
pub struct VaultEntry {
    #[id]
    pub id: i64,
    pub label: String,
    #[encrypted]
    pub secret: String,
    #[encrypted(deterministic)]
    pub lookup_key: String,
}

#[repository(VaultEntry, table = "vault_entries", versioned = true)]
pub trait VaultEntryRepository {}

#[test]
fn encrypted_columns_registered_and_versioned() {
    // The model registers its encrypted columns for composition.
    assert_eq!(VaultEntry::__AUTUMN_ENCRYPTED_COLUMNS, &["secret", "lookup_key"]);

    // Version history treats encrypted columns as sensitive automatically, so the
    // plaintext that the in-memory model would serialize never reaches the
    // version table — it is recorded as a "changed (encrypted)" marker instead.
    use autumn_web::version_history::VersionedRecord;
    let cols = VaultEntry::version_sensitive_columns();
    assert!(cols.contains(&"secret"), "encrypted column auto-sensitive: {cols:?}");
    assert!(cols.contains(&"lookup_key"), "encrypted column auto-sensitive: {cols:?}");
}
