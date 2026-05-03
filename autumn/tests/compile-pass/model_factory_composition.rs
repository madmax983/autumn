/// Verifies that `#[factory_assoc(Type)]` compiles and generates:
/// - An `Option<T>` field in the factory struct (None = auto-create in create())
/// - A `.user_id(val)` setter that stores `Some(val)`
/// - A `.user(pre_built)` setter that extracts the id from a pre-built instance

mod schema {
    autumn_web::reexports::diesel::table! {
        assoc_users (id) {
            id -> Int8,
            name -> Text,
        }
    }

    autumn_web::reexports::diesel::table! {
        assoc_posts (id) {
            id -> Int8,
            title -> Text,
            user_id -> Int8,
        }
    }
}

use schema::{assoc_posts, assoc_users};

#[autumn_web::model(table = "assoc_users")]
pub struct AssocUser {
    #[id]
    pub id: i64,
    pub name: String,
}

#[autumn_web::model(table = "assoc_posts")]
pub struct AssocPost {
    #[id]
    pub id: i64,
    pub title: String,
    #[factory_assoc(AssocUser)]
    pub user_id: i64,
}

fn main() {
    // --- AssocUser factory is unaffected ---
    let u: NewAssocUser = AssocUser::factory().name("Alice").build();
    assert_eq!(u.name, "Alice");

    // --- AssocPost factory with assoc field ---

    // user_id defaults to None (will auto-create on create())
    let p: NewAssocPost = AssocPost::factory().title("Hello").build();
    assert_eq!(p.title, "Hello");
    assert_eq!(p.user_id, 0); // unwrap_or_default() on None

    // Explicit user_id via setter
    let p = AssocPost::factory().title("World").user_id(42_i64).build();
    assert_eq!(p.user_id, 42);

    // Override with a pre-built user instance
    let built_user = AssocUser {
        id: 7,
        name: "Bob".into(),
    };
    let p = AssocPost::factory().user(&built_user).build();
    assert_eq!(p.user_id, 7); // extracted from built_user.id

    // Pre-built user does not consume the user (takes &AssocUser)
    let _ = built_user.id; // still accessible
}
