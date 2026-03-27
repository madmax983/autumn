// Compile-pass: existing #[repository] without hooks should still work unchanged

mod schema {
    autumn_web::reexports::diesel::table! {
        notes (id) {
            id -> Int4,
            content -> Text,
        }
    }
}

use schema::notes;

#[autumn_web::model]
pub struct Note {
    #[id]
    pub id: i32,
    pub content: String,
}

#[autumn_web::repository(Note)]
pub trait NoteRepository {
    fn find_by_content(content: String) -> Vec<Note>;
}

fn main() {}
