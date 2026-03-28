mod schema {
    autumn_web::reexports::diesel::table! {
        gadgets (id) {
            id -> Int8,
            name -> Text,
            status -> Text,
        }
    }
}

use schema::gadgets;
use autumn_web::prelude::*;

#[autumn_web::model]
pub struct Gadget {
    #[id]
    pub id: i64,
    pub name: String,
    pub status: String,
}

#[derive(Clone, Default)]
pub struct GadgetHooks;

impl MutationHooks for GadgetHooks {
    type Model = Gadget;
    type NewModel = NewGadget;
    type UpdateModel = UpdateGadget;
}

#[autumn_web::repository(Gadget, hooks = GadgetHooks, api = "/api/v1/gadgets")]
pub trait GadgetRepository {
    fn find_by_status(status: String) -> Vec<Gadget>;
}

fn main() {
    // Verify all 5 API handlers exist
    let _ = gadget_api_list;
    let _ = gadget_api_get;
    let _ = gadget_api_create;
    let _ = gadget_api_update;
    let _ = gadget_api_delete;
}
