mod schema {
    autumn_web::reexports::diesel::table! {
        accounts (id) {
            id -> Int8,
            email -> Text,
            display_name -> Text,
            password_hash -> Text,
            reset_token -> Nullable<Text>,
        }
    }
}

use autumn_web::prelude::*;
use schema::accounts;

mod display_name_adapter {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &String, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_uppercase())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)
    }
}

#[autumn_web::model]
pub struct Account {
    #[id]
    pub id: i64,
    pub email: String,
    #[serde(with = "display_name_adapter")]
    pub display_name: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    #[serde(skip)]
    pub reset_token: Option<String>,
}

#[derive(Clone, Default)]
pub struct AccountHooks;

impl MutationHooks for AccountHooks {
    type Model = Account;
    type NewModel = NewAccount;
    type UpdateModel = UpdateAccount;
}

#[autumn_web::repository(Account, hooks = AccountHooks)]
pub trait AccountRepository {}

fn main() {
    let account = Account {
        id: 1,
        email: "a@example.com".to_owned(),
        display_name: "Ada".to_owned(),
        password_hash: "hash".to_owned(),
        reset_token: Some("token".to_owned()),
    };
    let payload = account.__autumn_commit_hook_to_value().unwrap();
    let _roundtrip = Account::__autumn_commit_hook_from_value(payload).unwrap();
}
