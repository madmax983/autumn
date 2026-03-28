mod schema {
    autumn_web::reexports::diesel::table! {
        articles (id) {
            id -> Int8,
            title -> Text,
            slug -> Text,
            subtitle -> Nullable<Text>,
        }
    }
}

use schema::articles;
#[allow(unused_imports)]
use autumn_web::hooks::{Patch, UpdateDraft};

#[autumn_web::model]
pub struct Article {
    #[id]
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub subtitle: Option<String>,
}

fn main() {
    // Verify UpdateArticle uses Patch<T>
    let _update = UpdateArticle {
        title: Patch::Set("New Title".to_string()),
        slug: Patch::Unchanged,
        subtitle: Patch::Clear,
    };

    // Verify Default works
    let _default: UpdateArticle = Default::default();

    // Verify from_patch builds a draft (via the generated ArticleDraftExt trait)
    let current = Article {
        id: 1,
        title: "Old".into(),
        slug: "old".into(),
        subtitle: None,
    };
    let patch = UpdateArticle {
        title: Patch::Set("New".into()),
        slug: Patch::Unchanged,
        subtitle: Patch::Unchanged,
    };
    let mut draft = UpdateDraft::<Article>::from_patch(&current, &patch).unwrap();

    // Verify per-field accessors work
    assert!(draft.title().changed());
    assert!(draft.title().changed_to(&"New".to_string()));
    assert!(draft.slug().unchanged());

    // Verify set() mutates the draft
    draft.slug().set("new-slug".into());
    assert!(draft.slug().changed());

    // Verify after() reflects the change
    assert_eq!(draft.after().slug, "new-slug");

    // Verify Clear on nullable field sets to None
    let clear_patch = UpdateArticle {
        title: Patch::Unchanged,
        slug: Patch::Unchanged,
        subtitle: Patch::Clear,
    };
    let draft2 = UpdateDraft::<Article>::from_patch(&current, &clear_patch).unwrap();
    assert_eq!(draft2.after().subtitle, None);
}
