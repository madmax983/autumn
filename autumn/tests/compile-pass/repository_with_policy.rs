//! Compile-pass test: `#[repository(api = ..., policy = T, scope = S)]`
//! with valid `Policy<Model>` / `Scope<Model>` impls compiles cleanly.
//!
//! Companion to `compile-fail/repository_invalid_policy_type.rs`,
//! which pins that a typo in `policy = ...` is rejected at compile
//! time.

use autumn_web::authorization::{BoxFuture, Policy, PolicyContext, Scope};
use autumn_web::reexports::diesel_async::AsyncPgConnection;

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

#[derive(Default, Clone)]
pub struct WidgetScope;

impl Scope<Widget> for WidgetScope {
    fn list<'a>(
        &'a self,
        _ctx: &'a PolicyContext,
        _conn: &'a mut AsyncPgConnection,
    ) -> BoxFuture<'a, autumn_web::AutumnResult<Vec<Widget>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[autumn_web::repository(
    Widget,
    api = "/api/widgets",
    policy = WidgetPolicy,
    scope = WidgetScope,
)]
pub trait WidgetRepository {}

fn main() {
    let _ = widget_api_list;
    let _ = widget_api_get;
    let _ = widget_api_create;
    let _ = widget_api_update;
    let _ = widget_api_delete;
}
