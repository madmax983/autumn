//! Issue #835 / PR review: `preload` must apply the association target's own
//! read scoping (tenant isolation + soft-delete), mirroring what the target's
//! repository finders hide — and *only* when the target's repository actually
//! opts into that scoping.
//!
//! Scoping is keyed off the target repository's `#[repository(..., soft_delete,
//! tenant_scoped)]` config (surfaced to the `#[model]`-generated
//! `__autumn_preload_retain` via the `AutumnPreloadScopeExt` inherent-override
//! pattern), **not** off field presence: a model may carry a `deleted_at` /
//! `tenant_id` column (audit history, denormalized tenant) without its
//! repository scoping on it, and then finders — and preload — leave it
//! unfiltered. `across_tenants()` is honored via the ambient
//! `PRELOAD_ACROSS_TENANTS` task-local that a repository's `preload` publishes.
//!
//! These tests exercise the retain in-memory — no database required — by
//! calling the generated helper directly on hand-built rows.

#![cfg(feature = "db")]

use autumn_web::preload::PRELOAD_ACROSS_TENANTS;
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
        // Same columns as `scoped_items`, but its repository does NOT opt into
        // soft_delete / tenant_scoped — so preload must leave it unfiltered.
        audit_items (id) {
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

use schema::{audit_items, plain_items, scoped_items};

#[autumn_web::model(table = "scoped_items")]
pub struct ScopedItem {
    #[id]
    pub id: i64,
    pub name: String,
    pub tenant_id: String,
    #[default]
    pub deleted_at: Option<chrono::NaiveDateTime>,
}

// Repository opts into both scopes → preload scopes `ScopedItem`.
#[autumn_web::repository(ScopedItem, table = "scoped_items", soft_delete, tenant_scoped)]
pub trait ScopedItemRepository {}

#[autumn_web::model(table = "audit_items")]
pub struct AuditItem {
    #[id]
    pub id: i64,
    pub name: String,
    pub tenant_id: String,
    #[default]
    pub deleted_at: Option<chrono::NaiveDateTime>,
}

// Repository does NOT opt into soft_delete / tenant_scoped, even though the
// columns exist (e.g. `deleted_at` is audit history). Finders don't filter, so
// neither should preload.
#[autumn_web::repository(AuditItem, table = "audit_items")]
pub trait AuditItemRepository {}

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

fn audit(id: i64, tenant: &str, deleted: bool) -> AuditItem {
    AuditItem {
        id,
        name: format!("audit-{id}"),
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
    // context), but soft-deleted rows are always hidden (repo is soft_delete).
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

#[tokio::test]
async fn across_tenants_skips_tenant_filter_but_keeps_soft_delete() {
    // Under `across_tenants()` (ambient flag set by the repository's preload),
    // the tenant predicate is skipped at every level — but soft-delete still
    // applies, exactly like an `across_tenants()` finder.
    let rows = vec![
        item(1, "acme", false),   // kept (tenant filter skipped)
        item(2, "globex", false), // kept (tenant filter skipped)
        item(3, "acme", true),    // soft-deleted → dropped
    ];

    let kept = CURRENT_TENANT
        .scope(Some("acme".to_string()), async {
            PRELOAD_ACROSS_TENANTS
                .scope(true, async { ScopedItem::__autumn_preload_retain(rows) })
                .await
        })
        .await;

    assert_eq!(ids(&kept), vec![1, 2]);
}

#[tokio::test]
async fn retain_does_not_scope_when_repository_opts_out() {
    // `AuditItem` has `deleted_at` + `tenant_id` columns, but its repository is
    // not `soft_delete` / `tenant_scoped`. Finders leave these rows unfiltered,
    // so preload must too — even with a tenant in context and soft-deleted rows
    // present. (Regression test for the field-presence bug.)
    let rows = vec![
        audit(1, "acme", false),
        audit(2, "globex", false), // other tenant → still kept
        audit(3, "acme", true),    // "deleted" audit row → still kept
    ];

    let kept = CURRENT_TENANT
        .scope(Some("acme".to_string()), async {
            AuditItem::__autumn_preload_retain(rows)
        })
        .await;

    assert_eq!(kept.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2, 3]);
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
