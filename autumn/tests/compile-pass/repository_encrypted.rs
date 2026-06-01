// A full versioned repository over a model with encrypted columns must compile:
// insert, update, bulk, upsert, and version-history composition all work with
// the diesel serialize_as/deserialize_as wrappers.

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

fn main() {}
