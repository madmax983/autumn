// Compile-pass: tenant-scoped versioned repositories support Option<String> tenant IDs.

mod schema {
    autumn_web::reexports::diesel::table! {
        scoped_audit_notes (id) {
            id -> Int8,
            tenant_id -> Nullable<Text>,
            content -> Text,
        }
    }
}

use schema::scoped_audit_notes;

#[autumn_web::model]
pub struct ScopedAuditNote {
    #[id]
    pub id: i64,
    pub tenant_id: Option<String>,
    pub content: String,
}

#[autumn_web::repository(
    ScopedAuditNote,
    table = "scoped_audit_notes",
    tenant_scoped,
    versioned = true
)]
pub trait ScopedAuditNoteRepository {}

fn main() {}
