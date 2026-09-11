//! SQLite-backed [`DbSuppressionStore`] coverage (issue #2697).
//!
//! The store's `table!` used to declare the Postgres-only `Timestamptz` for
//! `unsubscribed_at`, which disagrees with the DDL `autumn generate mailer
//! --list-unsubscribe` emits for a SQLite app (`TEXT NOT NULL DEFAULT
//! CURRENT_TIMESTAMP`). No query in the store touches the column, so it
//! round-tripped at runtime by luck — but any future
//! `.select(unsubscribed_at)` or full-row `Queryable` would have broken the
//! `sqlite` build. The `table!` is now forked per backend (the same
//! convention as `notifications` and `push_subscriptions`).
//!
//! This exercises `suppress` → `is_suppressed` → `is_suppressed_many`
//! against the generator's OWN emitted SQLite DDL, with no Docker and no
//! testcontainers: a tempfile-backed database, so the writes and the reads
//! observe the same data regardless of how deadpool recycles connections.
//!
//! Run it explicitly (the `sqlite` backend flip cannot be enabled alongside
//! the Postgres lanes):
//!
//! ```sh
//! cargo test -p autumn-web --features "sqlite,test-support,mail" --test sqlite_mail_suppression
//! ```
#![cfg(all(feature = "sqlite", feature = "mail"))]

use std::collections::HashSet;

use autumn_web::config::DatabaseConfig;
use autumn_web::db::{RuntimeConnection, create_pool};
use autumn_web::mail::SuppressionStore;
use autumn_web::mail::db_suppression::DbSuppressionStore;
use autumn_web::reexports::{chrono, diesel, diesel_async};

use chrono::{DateTime, Utc};
use diesel_async::RunQueryDsl as _;
use diesel_async::pooled_connection::deadpool::Pool;

/// The runtime pool type. Under `--features sqlite` `RuntimeConnection`
/// resolves to `SyncConnectionWrapper<SqliteConnection>`.
type SqlitePool = Pool<RuntimeConnection>;

/// Matches the DDL emitted by `autumn generate mailer --list-unsubscribe` for
/// the SQLite backend (autumn-cli `generate/mailer.rs`,
/// `UNSUBSCRIBE_MIGRATION_UP_SQLITE`). Keep the two in sync.
const CREATE_MAIL_UNSUBSCRIBES_SQLITE: &str = "CREATE TABLE mail_unsubscribes (\
     id INTEGER PRIMARY KEY AUTOINCREMENT, \
     subscriber TEXT NOT NULL, \
     list_id TEXT NOT NULL, \
     unsubscribed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, \
     UNIQUE (subscriber, list_id))";

/// A [`DbSuppressionStore`] over the generator's emitted SQLite DDL, plus the
/// pool (for direct assertions) and the temp dir (which must outlive both).
async fn setup() -> (DbSuppressionStore, SqlitePool, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let db_path = tmp.path().join("suppression.db");
    let url = format!("sqlite://{}", db_path.display());

    let config = DatabaseConfig {
        url: Some(url),
        ..Default::default()
    };
    let pool: SqlitePool = create_pool(&config)
        .expect("sqlite pool builds")
        .expect("a url is configured");

    {
        let mut conn = pool.get().await.expect("checkout a sqlite connection");
        diesel::sql_query(CREATE_MAIL_UNSUBSCRIBES_SQLITE)
            .execute(&mut *conn)
            .await
            .expect("create mail_unsubscribes from the generator's SQLite DDL");
    }

    (DbSuppressionStore::new(pool.clone()), pool, tmp)
}

#[tokio::test]
async fn sqlite_suppress_then_is_suppressed_round_trips() {
    let (store, _pool, _tmp) = setup().await;

    assert!(
        !store
            .is_suppressed("ada@example.com", "weekly_digest")
            .await
            .expect("is_suppressed before suppress"),
        "nothing is suppressed before the first suppress"
    );

    store
        .suppress("ada@example.com", "weekly_digest")
        .await
        .expect("suppress");

    assert!(
        store
            .is_suppressed("ada@example.com", "weekly_digest")
            .await
            .expect("is_suppressed after suppress")
    );
    // Other lists and other subscribers are unaffected.
    assert!(
        !store
            .is_suppressed("ada@example.com", "monthly_roundup")
            .await
            .expect("other list")
    );
    assert!(
        !store
            .is_suppressed("grace@example.com", "weekly_digest")
            .await
            .expect("other subscriber")
    );
}

#[tokio::test]
async fn sqlite_suppress_is_idempotent() {
    let (store, pool, _tmp) = setup().await;

    store
        .suppress("ada@example.com", "weekly_digest")
        .await
        .expect("first suppress");
    // The `(subscriber, list_id)` UNIQUE constraint plus `ON CONFLICT DO
    // NOTHING` makes the second write a no-op instead of an error.
    store
        .suppress("ada@example.com", "weekly_digest")
        .await
        .expect("second suppress is a no-op");

    assert!(
        store
            .is_suppressed("ada@example.com", "weekly_digest")
            .await
            .expect("still suppressed")
    );

    // Exactly one row — the conflict path did not insert a duplicate.
    let mut conn = pool.get().await.expect("checkout");
    #[derive(diesel::QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let row: CountRow = diesel::sql_query(
        "SELECT COUNT(*) AS n FROM mail_unsubscribes WHERE subscriber = 'ada@example.com'",
    )
    .get_result(&mut *conn)
    .await
    .expect("count rows");
    assert_eq!(row.n, 1, "idempotent suppress leaves exactly one row");
}

#[tokio::test]
async fn sqlite_is_suppressed_many_partitions_by_list() {
    let (store, _pool, _tmp) = setup().await;

    store
        .suppress("ada@example.com", "weekly_digest")
        .await
        .expect("suppress ada on weekly_digest");
    store
        .suppress("grace@example.com", "monthly_roundup")
        .await
        .expect("suppress grace on another list");

    let hits: HashSet<String> = store
        .is_suppressed_many(
            &["ada@example.com", "grace@example.com", "hopper@example.com"],
            "weekly_digest",
        )
        .await
        .expect("is_suppressed_many");

    assert_eq!(
        hits,
        ["ada@example.com".to_owned()].into_iter().collect(),
        "only the (subscriber, list_id) pair that was suppressed is returned"
    );
}

#[tokio::test]
async fn sqlite_defaulted_unsubscribed_at_reads_back_through_timestamptz_sqlite() {
    let (store, pool, _tmp) = setup().await;

    store
        .suppress("ada@example.com", "weekly_digest")
        .await
        .expect("suppress");

    // `suppress` never binds `unsubscribed_at`, so the emitted DDL's
    // `DEFAULT CURRENT_TIMESTAMP` filled it. Reading it back through the
    // forked `TimestamptzSqlite` mapping proves the declared column type and
    // the generator's emitted DDL agree at runtime, not just at the text
    // level (diesel parses SQLite's `YYYY-MM-DD HH:MM:SS` into `DateTime<Utc>`
    // via its naive-datetime fallback).
    let mut conn = pool.get().await.expect("checkout");
    #[derive(diesel::QueryableByName)]
    struct DefaultedRow {
        #[diesel(sql_type = diesel::sql_types::TimestamptzSqlite)]
        unsubscribed_at: DateTime<Utc>,
    }
    let row: DefaultedRow = diesel::sql_query("SELECT unsubscribed_at FROM mail_unsubscribes")
        .get_result(&mut *conn)
        .await
        .expect("read defaulted unsubscribed_at through TimestamptzSqlite");
    assert!(
        row.unsubscribed_at <= Utc::now(),
        "the DB default filled unsubscribed_at with a sane timestamp"
    );
}
