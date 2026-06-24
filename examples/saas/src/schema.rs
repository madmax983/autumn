// Diesel table definitions. These mirror `migrations/`.

diesel::table! {
    users (id) {
        id -> Int8,
        email -> Text,
        password_hash -> Text,
        tenant_id -> Text,
        created_at -> Timestamp,
    }
}

diesel::table! {
    projects (id) {
        id -> Int8,
        tenant_id -> Text,
        name -> Text,
        created_at -> Timestamp,
    }
}
