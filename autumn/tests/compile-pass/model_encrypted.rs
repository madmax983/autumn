// `#[encrypted]` / `#[encrypted(deterministic)]` field attribute on `#[model]`.
// The model fields stay plain `String`; the macro routes them through the AEAD
// wrapper via diesel serialize_as/deserialize_as and registers the columns.

use autumn_web::model;

diesel::table! {
    accounts (id) {
        id -> Integer,
        email -> Text,
        api_token -> Text,
        note -> Text,
    }
}

#[model(table = "accounts")]
pub struct Account {
    pub id: i32,
    // Deterministic: supports equality lookups (tradeoff named in docs).
    #[encrypted(deterministic)]
    pub email: String,
    // Randomized (default): no equality lookups.
    #[encrypted]
    pub api_token: String,
    // Plain column, unchanged.
    pub note: String,
}

fn main() {
    // The encrypted columns are registered for redaction/scrub composition.
    assert_eq!(Account::__AUTUMN_ENCRYPTED_COLUMNS, &["email", "api_token"]);
}
