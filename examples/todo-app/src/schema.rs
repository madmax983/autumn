diesel::table! {
    todos (id) {
        id -> Int8,
        title -> Text,
        completed -> Bool,
        created_at -> Timestamp,
    }
}
