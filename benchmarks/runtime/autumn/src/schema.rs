diesel::table! {
    posts (id) {
        id -> Int8,
        title -> Text,
        body -> Text,
        published -> Bool,
        author -> Text,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    api_tokens (id) {
        id -> Int8,
        token -> Text,
        principal -> Text,
        created_at -> Timestamptz,
    }
}
