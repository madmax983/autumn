//! Compile-pass test: `#[repository(api = ..., policy = T)]`
//! with a valid `Policy<Model>` impl compiles cleanly.
//!
//! Companion to `compile-fail/repository_invalid_policy_type.rs`,
//! which pins that a typo in `policy = ...` is rejected at compile
//! time.

use autumn_web::authorization::Policy;

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

#[derive(Default, Clone)]
pub struct WidgetPolicy;

impl Policy<Widget> for WidgetPolicy {}

#[autumn_web::repository(
    Widget,
    api = "/api/widgets",
    policy = WidgetPolicy,
)]
pub trait WidgetRepository {}

fn main() {
    let _ = widget_api_list;
    let _ = widget_api_get;
    let _ = widget_api_create;
    let _ = widget_api_update;
    let _ = widget_api_delete;
}
