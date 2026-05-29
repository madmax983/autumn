// Compile-pass: #[repository(versioned = true)] emits upsert_many history locking code.

mod schema {
    autumn_web::reexports::diesel::table! {
        audit_notes (id) {
            id -> Int8,
            content -> Text,
        }
    }
}

use schema::audit_notes;

#[autumn_web::model]
pub struct AuditNote {
    #[id]
    pub id: i64,
    pub content: String,
}

#[autumn_web::repository(AuditNote, table = "audit_notes", versioned = true)]
pub trait AuditNoteRepository {}

fn main() {}
