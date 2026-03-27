diesel::table! {
    posts (id) {
        id -> Int4,
        title -> Text,
        slug -> Text,
        body -> Text,
        published -> Bool,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}
