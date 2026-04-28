//! Compile-fail test: `policy = ...` referencing a type that doesn't
//! `impl Policy<Model>` is rejected at compile time.
//!
//! Without the compile-time assertion in `#[repository]`, a typo or
//! a real type that does not implement `Policy<Model>` would silently
//! compile and only fail at request time with `500 missing policy
//! registration`.

mod schema {
    autumn_web::reexports::diesel::table! {
        widgets (id) {
            id -> Int8,
            name -> Text,
        }
    }
}

use schema::widgets;

#[autumn_web::model]
pub struct Widget {
    #[id]
    pub id: i64,
    pub name: String,
}

// Real type that intentionally does NOT implement `Policy<Widget>`.
pub struct NotAPolicy;

#[autumn_web::repository(Widget, api = "/api/widgets", policy = NotAPolicy)]
pub trait WidgetRepository {}

fn main() {}
