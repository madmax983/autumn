//! Integration tests for declarative live broadcast via `#[repository(Model, broadcasts = "topic")]`.
//!
//! These tests verify that:
//! - `save` publishes an OOB insert fragment to the configured topic
//! - `update` publishes an OOB outerHTML swap fragment
//! - `delete_by_id` publishes an OOB delete fragment
//! - Repositories declared without `broadcasts` emit nothing
//! - Repositories constructed without state (via `with_pool_untracked`) skip broadcast silently
//!
//! **Requires Docker** (Postgres testcontainer) for DB-backed tests.

#![cfg(all(feature = "ws", feature = "maud", feature = "htmx", feature = "db"))]
#![allow(clippy::must_use_candidate, clippy::missing_const_for_fn)]

use autumn_web::channels::Channels;
use autumn_web::hooks::Patch;
use autumn_web::live::LiveFragment;
use autumn_web::prelude::*;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ── Schema ────────────────────────────────────────────────────────────────────

diesel::table! {
    live_items (id) {
        id -> Int8,
        name -> Text,
    }
}

// ── Model ─────────────────────────────────────────────────────────────────────

#[autumn_web::model(table = "live_items")]
#[derive(PartialEq, Eq)]
pub struct LiveItem {
    #[id]
    pub id: i64,
    pub name: String,
}

// ── LiveFragment impl ─────────────────────────────────────────────────────────

impl LiveFragment for LiveItem {
    fn dom_id_for(id: i64) -> String {
        format!("live-item-{id}")
    }

    fn dom_id(&self) -> String {
        Self::dom_id_for(self.id)
    }

    fn render_fragment(&self) -> maud::Markup {
        html! {
            li id=(self.dom_id()) { (self.name) }
        }
    }
}

// ── Repository with broadcasts ────────────────────────────────────────────────

#[autumn_web::repository(LiveItem, table = "live_items", broadcasts = "live_items")]
pub trait LiveItemRepository {}

// ── Repository WITHOUT broadcasts (control group) ────────────────────────────

#[autumn_web::model(table = "live_items")]
#[derive(PartialEq, Eq)]
pub struct SilentItem {
    #[id]
    pub id: i64,
    pub name: String,
}

#[autumn_web::repository(SilentItem, table = "live_items")]
pub trait SilentItemRepository {}

// ── Test helpers ──────────────────────────────────────────────────────────────

async fn setup_db() -> Pool<AsyncPgConnection> {
    let container = Postgres::default().start().await.expect("postgres start");
    let url = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        container.get_host().await.unwrap(),
        container.get_host_port_ipv4(5432).await.unwrap(),
    );
    let manager = AsyncDieselConnectionManager::new(url);
    let pool = Pool::builder(manager).build().expect("pool build");
    let mut conn = pool.get().await.unwrap();
    let _: diesel::QueryResult<usize> = diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS live_items (id BIGSERIAL PRIMARY KEY, name TEXT NOT NULL)",
    )
    .execute(&mut *conn)
    .await;
    drop(conn);
    pool
}

/// Build a repository backed by the given pool AND a channels handle so
/// broadcasts fire. Simulates what `FromRequestParts<AppState>` does.
fn repo_with_broadcast(pool: Pool<AsyncPgConnection>, channels: &Channels) -> PgLiveItemRepository {
    let broadcast = channels.broadcast();
    PgLiveItemRepository::__autumn_test_with_broadcast(pool, broadcast)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn save_broadcasts_oob_fragment() {
    let pool = setup_db().await;
    let channels = Channels::new(16);
    let mut rx = channels.subscribe("live_items");
    let repo = repo_with_broadcast(pool, &channels);

    let new_item = NewLiveItem {
        name: "hello".to_owned(),
    };
    let saved = repo.save(&new_item).await.expect("save");

    let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .expect("broadcast timed out")
        .expect("channel closed");

    let html = msg.as_str();
    assert!(
        html.contains("hx-swap-oob"),
        "must contain hx-swap-oob, got: {html}"
    );
    assert!(
        html.contains(&LiveItem::dom_id_for(saved.id)),
        "must contain dom_id, got: {html}"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn update_broadcasts_true_swap() {
    let pool = setup_db().await;
    let channels = Channels::new(16);
    let mut rx = channels.subscribe("live_items");
    let repo = repo_with_broadcast(pool, &channels);

    let saved = repo
        .save(&NewLiveItem {
            name: "first".to_owned(),
        })
        .await
        .expect("save");
    // drain the insert broadcast
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;

    repo.update(
        saved.id,
        &UpdateLiveItem {
            name: Patch::Set("updated".to_owned()),
        },
    )
    .await
    .expect("update");

    let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .expect("update broadcast timed out")
        .expect("channel closed");

    let html = msg.as_str();
    assert!(
        html.contains("hx-swap-oob"),
        "must contain hx-swap-oob: {html}"
    );
    assert!(
        html.contains(&LiveItem::dom_id_for(saved.id)),
        "must contain dom_id: {html}"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn delete_broadcasts_oob_delete() {
    let pool = setup_db().await;
    let channels = Channels::new(16);
    let mut rx = channels.subscribe("live_items");
    let repo = repo_with_broadcast(pool, &channels);

    let saved = repo
        .save(&NewLiveItem {
            name: "to_delete".to_owned(),
        })
        .await
        .expect("save");
    // drain the insert broadcast
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;

    repo.delete_by_id(saved.id).await.expect("delete");

    let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .expect("delete broadcast timed out")
        .expect("channel closed");

    let html = msg.as_str();
    assert!(html.contains("delete"), "must contain delete swap: {html}");
    assert!(
        html.contains(&LiveItem::dom_id_for(saved.id)),
        "must contain dom_id: {html}"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn no_broadcasts_attr_emits_nothing() {
    let pool = setup_db().await;
    let channels = Channels::new(16);
    let mut rx = channels.subscribe("live_items");

    let repo = PgSilentItemRepository::with_pool_untracked(pool);

    let new_item = NewSilentItem {
        name: "quiet".to_owned(),
    };
    repo.save(&new_item).await.expect("save");

    let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
    assert!(result.is_err(), "no broadcasts should have been emitted");
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn with_pool_untracked_skips_broadcast_silently() {
    let pool = setup_db().await;
    let channels = Channels::new(16);
    let mut rx = channels.subscribe("live_items");

    // Construct without state — broadcast channel is None
    let repo = PgLiveItemRepository::with_pool_untracked(pool);
    let new_item = NewLiveItem {
        name: "no_state".to_owned(),
    };
    let _saved = repo
        .save(&new_item)
        .await
        .expect("save succeeds even without broadcast");

    let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
    assert!(
        result.is_err(),
        "with_pool_untracked must not panic or broadcast"
    );
}
