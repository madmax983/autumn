//! Issue #835 / PR review: `preload` must apply the association target's own
//! read scoping (tenant isolation + soft-delete), mirroring what the target's
//! repository finders hide. `#[model]` generates `__autumn_preload_retain`,
//! which the generated `load_associations` calls on freshly loaded target rows.
//!
//! These tests exercise that scoping in-memory — no database required — by
//! calling the generated helper directly on hand-built rows.

#![cfg(feature = "db")]

use autumn_web::tenancy::CURRENT_TENANT;

mod schema {
    autumn_web::reexports::diesel::table! {
        scoped_items (id) {
            id -> Int8,
            name -> Text,
            tenant_id -> Text,
            deleted_at -> Nullable<Timestamp>,
        }
    }

    autumn_web::reexports::diesel::table! {
        plain_items (id) {
            id -> Int8,
            name -> Text,
        }
    }
}

use schema::{plain_items, scoped_items};

#[autumn_web::model(table = "scoped_items")]
pub struct ScopedItem {
    #[id]
    pub id: i64,
    pub name: String,
    pub tenant_id: String,
    #[default]
    pub deleted_at: Option<chrono::NaiveDateTime>,
}

#[autumn_web::model(table = "plain_items")]
pub struct PlainItem {
    #[id]
    pub id: i64,
    pub name: String,
}

fn item(id: i64, tenant: &str, deleted: bool) -> ScopedItem {
    ScopedItem {
        id,
        name: format!("item-{id}"),
        tenant_id: tenant.to_string(),
        deleted_at: deleted.then(chrono::NaiveDateTime::default),
    }
}

fn ids(rows: &[ScopedItem]) -> Vec<i64> {
    rows.iter().map(|r| r.id).collect()
}

#[tokio::test]
async fn retain_keeps_only_current_tenant_and_drops_soft_deleted() {
    let rows = vec![
        item(1, "acme", false),   // kept
        item(2, "globex", false), // other tenant → dropped
        item(3, "acme", true),    // soft-deleted → dropped
        item(4, "acme", false),   // kept
    ];

    let kept = CURRENT_TENANT
        .scope(Some("acme".to_string()), async move {
            ScopedItem::__autumn_preload_retain(rows)
        })
        .await;

    assert_eq!(ids(&kept), vec![1, 4]);
}

#[tokio::test]
async fn retain_without_tenant_context_still_drops_soft_deleted() {
    // No CURRENT_TENANT set: tenant filtering is skipped (single-tenant / admin
    // context), but soft-deleted rows are always hidden.
    let rows = vec![
        item(1, "acme", false),
        item(2, "globex", true), // soft-deleted → dropped regardless of tenant
        item(3, "globex", false),
    ];

    let kept = ScopedItem::__autumn_preload_retain(rows);
    assert_eq!(ids(&kept), vec![1, 3]);
}

#[tokio::test]
async fn retain_isolates_each_tenant() {
    let make = || {
        vec![
            item(1, "acme", false),
            item(2, "globex", false),
            item(3, "acme", false),
        ]
    };

    let acme = CURRENT_TENANT
        .scope(Some("acme".to_string()), async {
            ScopedItem::__autumn_preload_retain(make())
        })
        .await;
    assert_eq!(ids(&acme), vec![1, 3]);

    let globex = CURRENT_TENANT
        .scope(Some("globex".to_string()), async {
            ScopedItem::__autumn_preload_retain(make())
        })
        .await;
    assert_eq!(ids(&globex), vec![2]);
}

#[test]
fn retain_is_identity_for_models_without_scoping_columns() {
    let rows = vec![
        PlainItem {
            id: 1,
            name: "a".into(),
        },
        PlainItem {
            id: 2,
            name: "b".into(),
        },
    ];
    let kept = PlainItem::__autumn_preload_retain(rows);
    assert_eq!(kept.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2]);
}
