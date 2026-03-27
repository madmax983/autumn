diesel::table! {
    bookmarks (id) {
        id -> Int4,
        url -> Text,
        title -> Text,
        tag -> Text,
        alive -> Bool,
        created_at -> Timestamp,
    }
}
