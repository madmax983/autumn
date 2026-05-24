// compile-fail: upsert_many is not supported when repository hooks are configured

mod schema {
    autumn_web::reexports::diesel::table! {
        items (id) {
            id -> Int8,
            name -> Text,
        }
    }
}

use schema::items;
use autumn_web::prelude::*;
use autumn_web::hooks::{MutationContext, MutationHooks};

#[autumn_web::model(table = "items")]
pub struct Item {
    #[id]
    pub id: i64,
    pub name: String,
}

pub struct DummyHooks;

impl MutationHooks for DummyHooks {
    type Model = Item;
    type NewModel = NewItem;
    type UpdateModel = UpdateItem;
}

#[autumn_web::repository(Item, table = "items", hooks = DummyHooks)]
pub trait ItemRepository {}

async fn test_compile_fail(repo: PgItemRepository) {
    let records = vec![
        Item {
            id: 1,
            name: "Test".to_string(),
        }
    ];
    // This should fail to compile because upsert_many is not generated for hooked repositories
    let _ = repo.upsert_many(&records).await;
}

fn main() {}
