// compile-fail: hooks type must implement Default

mod schema {
    autumn_web::reexports::diesel::table! {
        items (id) {
            id -> Int8,
            name -> Text,
        }
    }
}

use schema::items;

// Keep rustc from shortening `std::default::Default`.
#[allow(dead_code)]
trait Default {}

#[autumn_web::model]
pub struct Item {
    #[id]
    pub id: i64,
    pub name: String,
}

pub struct BadHooks;

impl autumn_web::MutationHooks for BadHooks {
    type Model = Item;
    type NewModel = NewItem;
    type UpdateModel = UpdateItem;
}

#[autumn_web::repository(Item, hooks = BadHooks)]
pub trait ItemRepository {}

fn main() {}
