//! Compile-pass regression: `#[repository(api = ..., policy = T)]`
//! must not require hand-written `NewModel` insert DTOs to implement
//! `Serialize`.

use autumn_web::authorization::Policy;
use serde::Deserialize;

mod schema {
    autumn_web::reexports::diesel::table! {
        widgets (id) {
            id -> Int8,
            name -> Text,
        }
    }
}

use schema::widgets;

#[derive(autumn_web::reexports::diesel::Queryable)]
#[derive(autumn_web::reexports::diesel::Selectable)]
#[derive(autumn_web::reexports::diesel::Insertable)]
#[derive(autumn_web::reexports::diesel::AsChangeset)]
#[derive(serde::Serialize, serde::Deserialize)]
#[diesel(table_name = widgets)]
pub struct Widget {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Deserialize)]
#[derive(autumn_web::reexports::diesel::Insertable)]
#[diesel(table_name = widgets)]
pub struct NewWidget {
    pub name: String,
}

#[derive(Clone, Deserialize)]
pub struct UpdateWidget {
    pub name: Option<String>,
}

#[derive(autumn_web::reexports::diesel::AsChangeset)]
#[diesel(table_name = widgets)]
pub struct UpdateWidgetChangeset {
    pub name: Option<String>,
}

impl UpdateWidget {
    #[doc(hidden)]
    pub fn __to_changeset(&self) -> UpdateWidgetChangeset {
        UpdateWidgetChangeset {
            name: self.name.clone(),
        }
    }
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
    let _ = widget_api_create;
}
