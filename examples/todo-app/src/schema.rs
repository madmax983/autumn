diesel::table! {
    todos (id) {
        id -> Int4,
        title -> Text,
        completed -> Bool,
        created_at -> Timestamp,
    }
}
