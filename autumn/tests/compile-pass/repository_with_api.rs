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

#[autumn_web::repository(Widget, api = "/api/widgets")]
pub trait WidgetRepository {}

fn main() {
    // Verify all 5 handler functions were generated
    let _ = widget_api_list;
    let _ = widget_api_get;
    let _ = widget_api_create;
    let _ = widget_api_update;
    let _ = widget_api_delete;
}
