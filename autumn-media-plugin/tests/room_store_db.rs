//! Postgres-backed integration tests for the DB-backed `RoomStore`
//! (`autumn_media_plugin::rooms_db::DbRoomStore`, epic #1974).
//!
//! Spins up a real Postgres container via testcontainers and exercises the
//! store's full lifecycle — create → join → roster → leave persistence, roster
//! member-gating, cross-instance sharing (the multi-process property), the
//! heartbeat's persisted liveness/expiry renewal, and the last-write-wins
//! `reap_stale` sweep — exactly mirroring
//! `autumn-admin-plugin/tests/token_admin_db.rs`.
//!
//! **Requires Docker.** These tests are `#[ignore]`d so a default `cargo test`
//! never needs a daemon; run them with `cargo test -p autumn-media-plugin --
//! --ignored`.
//!
//! The store's queries are written against autumn-web's `RuntimeConnection`
//! alias, which is `AsyncPgConnection` in the default (Postgres) build — the
//! backend this container provides — so `DbRoomStore` accepts the pool built
//! here directly. The `SQLite` lane compiles against the same alias but is not
//! exercised here (a `SQLite` test harness would require flipping the whole
//! build graph's `sqlite` feature; the queries are backend-portable by
//! construction).

use std::sync::Arc;

use autumn_media_plugin::rooms::{RoomError, RoomStore};
use autumn_media_plugin::rooms_db::DbRoomStore;
use chrono::{Duration, Utc};
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

/// Matches `migrations/20260720000000_media_rooms/up.sql` (portable TIMESTAMP
/// columns, composite keys, cascade + sweep indexes).
const CREATE_TABLES_SQL: &str = "
    CREATE TABLE IF NOT EXISTS media_rooms (
        namespace         TEXT      NOT NULL,
        room_id           TEXT      NOT NULL,
        max_participants  INTEGER   NOT NULL,
        created_at        TIMESTAMP NOT NULL,
        PRIMARY KEY (namespace, room_id)
    );
    CREATE TABLE IF NOT EXISTS media_room_participants (
        namespace         TEXT      NOT NULL,
        room_id           TEXT      NOT NULL,
        participant_id    TEXT      NOT NULL,
        display_name      TEXT,
        token             TEXT      NOT NULL,
        joined_at         TIMESTAMP NOT NULL,
        token_expires_at  TIMESTAMP NOT NULL,
        last_seen_at      TIMESTAMP NOT NULL,
        PRIMARY KEY (namespace, room_id, participant_id),
        FOREIGN KEY (namespace, room_id)
            REFERENCES media_rooms (namespace, room_id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS media_room_participants_last_seen_idx
        ON media_room_participants (last_seen_at);
    CREATE INDEX IF NOT EXISTS media_rooms_created_at_idx
        ON media_rooms (created_at);
";

async fn setup_pool() -> (
    Pool<AsyncPgConnection>,
    testcontainers::ContainerAsync<Postgres>,
) {
    let container = Postgres::default()
        .start()
        .await
        .expect("failed to start postgres container");
    let host = container.get_host().await.expect("host");
    let port = container.get_host_port_ipv4(5432).await.expect("port");

    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(&url);
    let pool = Pool::builder(manager).max_size(5).build().expect("pool");

    let mut conn = pool.get().await.expect("conn");
    diesel::sql_query("DROP TABLE IF EXISTS media_room_participants")
        .execute(&mut conn)
        .await
        .expect("drop participants");
    diesel::sql_query("DROP TABLE IF EXISTS media_rooms")
        .execute(&mut conn)
        .await
        .expect("drop rooms");
    // The multi-statement DDL string is split so each runs as its own query.
    for stmt in CREATE_TABLES_SQL.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        diesel::sql_query(stmt)
            .execute(&mut conn)
            .await
            .expect("create table");
    }

    (pool, container)
}

/// Seed a room and its participants with explicit timestamps via raw SQL, so the
/// reaper tests can control `created_at` / `last_seen_at` deterministically
/// (mirroring the in-memory `seed_room` helper).
async fn seed(
    pool: &Pool<AsyncPgConnection>,
    namespace: &str,
    room_id: &str,
    created_at: chrono::DateTime<Utc>,
    seats: &[(&str, &str, chrono::DateTime<Utc>)],
) {
    let mut conn = pool.get().await.expect("conn");
    let created = created_at.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f");
    diesel::sql_query(format!(
        "INSERT INTO media_rooms (namespace, room_id, max_participants, created_at) \
         VALUES ('{namespace}', '{room_id}', 6, '{created}')"
    ))
    .execute(&mut conn)
    .await
    .expect("seed room");
    for (id, token, last_seen) in seats {
        let seen = last_seen.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f");
        diesel::sql_query(format!(
            "INSERT INTO media_room_participants \
             (namespace, room_id, participant_id, display_name, token, joined_at, token_expires_at, last_seen_at) \
             VALUES ('{namespace}', '{room_id}', '{id}', NULL, '{token}', '{seen}', '{seen}', '{seen}')"
        ))
        .execute(&mut conn)
        .await
        .expect("seed participant");
    }
}

/// One `token_expires_at` column, for reading a renewal back out of the row.
#[derive(diesel::QueryableByName)]
struct ExpiryRow {
    #[diesel(sql_type = diesel::sql_types::Timestamp)]
    token_expires_at: chrono::NaiveDateTime,
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn create_join_roster_leave_round_trips_and_persists() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);

    // Create.
    let room = store.create_room("tenant-a", 4).await.expect("create");
    assert_eq!(room.max_participants, 4);
    assert!(room.participants.is_empty());
    assert_eq!(room.namespace, "tenant-a");

    // Join two participants.
    let first = store
        .join_room(
            "tenant-a",
            &room.id,
            Some("Ada".to_owned()),
            Duration::seconds(300),
        )
        .await
        .expect("join first");
    let second = store
        .join_room(
            "tenant-a",
            &room.id,
            Some("Grace".to_owned()),
            Duration::seconds(300),
        )
        .await
        .expect("join second");
    assert_ne!(first.participant_id, second.participant_id);
    assert!(!first.token.expose().is_empty());

    // Roster (member-gated) reflects both joins and persisted display names.
    let roster = store
        .roster("tenant-a", &room.id, first.token.expose())
        .await
        .expect("roster");
    assert_eq!(roster.participants.len(), 2);
    assert!(
        roster
            .participants
            .iter()
            .any(|p| p.display_name.as_deref() == Some("Grace"))
    );

    // A brand-new store instance over the SAME pool sees the SAME room — the
    // multi-process property the whole feature exists for.
    let other: Arc<dyn RoomStore> = Arc::new(DbRoomStore::new(pool.clone(), 6));
    let roster2 = other
        .roster("tenant-a", &room.id, second.token.expose())
        .await
        .expect("second instance roster");
    assert_eq!(roster2.participants.len(), 2);

    // Leave removes the seat.
    store
        .leave_room(
            "tenant-a",
            &room.id,
            &first.participant_id,
            first.token.expose(),
        )
        .await
        .expect("leave");
    let roster3 = store
        .roster("tenant-a", &room.id, second.token.expose())
        .await
        .expect("roster after leave");
    assert_eq!(roster3.participants.len(), 1);
    assert_eq!(roster3.participants[0].id, second.participant_id);
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn roster_is_member_gated_fail_closed() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool, 6);

    let room = store.create_room("", 4).await.expect("create");
    store
        .join_room("", &room.id, None, Duration::seconds(300))
        .await
        .expect("join");

    // No token / a wrong token resolve to the SAME error a nonexistent room
    // returns — no membership oracle.
    assert!(matches!(
        store.roster("", &room.id, "").await,
        Err(RoomError::RoomNotFound)
    ));
    assert!(matches!(
        store.roster("", &room.id, "not-a-member").await,
        Err(RoomError::RoomNotFound)
    ));
    // Wrong namespace never leaks the room.
    assert!(matches!(
        store.roster("other", &room.id, "").await,
        Err(RoomError::RoomNotFound)
    ));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn join_enforces_capacity_and_leave_drops_empty_room() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool, 6);

    let room = store.create_room("", 1).await.expect("create");
    let only = store
        .join_room("", &room.id, None, Duration::seconds(300))
        .await
        .expect("join");
    // The room is now full (cap 1).
    assert!(matches!(
        store
            .join_room("", &room.id, None, Duration::seconds(300))
            .await,
        Err(RoomError::RoomFull { max: 1 })
    ));
    // The last participant leaving drops the room entirely.
    store
        .leave_room("", &room.id, &only.participant_id, only.token.expose())
        .await
        .expect("leave");
    // Rejoining a dropped room is RoomNotFound.
    assert!(matches!(
        store
            .join_room("", &room.id, None, Duration::seconds(300))
            .await,
        Err(RoomError::RoomNotFound)
    ));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn reap_evicts_stale_participant_and_drops_emptied_room() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let now = Utc::now();
    let ttl = Duration::minutes(30);

    // room-keep: one stale + one fresh seat → stale evicted, room survives.
    seed(
        &pool,
        "",
        "room-keep",
        now - Duration::hours(2),
        &[
            ("stale", "tok-stale", now - Duration::hours(1)),
            ("fresh", "tok-fresh", now - Duration::minutes(5)),
        ],
    )
    .await;
    // room-drop: only a stale seat → emptied by reaping and dropped.
    seed(
        &pool,
        "",
        "room-drop",
        now - Duration::hours(2),
        &[("stale", "tok", now - Duration::hours(1))],
    )
    .await;

    let stats = store.reap_stale(now, ttl).await;
    assert_eq!(stats.participants_reaped, 2);
    assert_eq!(stats.rooms_reaped, 1);

    // room-keep still resolves for its fresh member; the stale one is gone.
    assert!(store.roster("", "room-keep", "tok-fresh").await.is_ok());
    assert!(matches!(
        store.roster("", "room-keep", "tok-stale").await,
        Err(RoomError::RoomNotFound)
    ));
    // room-drop is gone entirely.
    assert!(matches!(
        store.roster("", "room-drop", "tok").await,
        Err(RoomError::RoomNotFound)
    ));

    // Idempotent / last-write-wins: a second sweep reaps nothing.
    let again = store.reap_stale(now, ttl).await;
    assert_eq!(again.participants_reaped, 0);
    assert_eq!(again.rooms_reaped, 0);
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn reap_drops_created_never_joined_room_but_keeps_a_fresh_empty_room() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let now = Utc::now();
    let ttl = Duration::minutes(30);

    seed(&pool, "", "old-empty", now - Duration::hours(1), &[]).await;
    seed(&pool, "", "new-empty", now - Duration::minutes(5), &[]).await;

    let stats = store.reap_stale(now, ttl).await;
    assert_eq!(stats.participants_reaped, 0);
    assert_eq!(stats.rooms_reaped, 1);

    // A just-created empty room within the TTL survives (create→first-join
    // window is never reaped out from under a joiner); an old empty one is gone.
    let fresh = store
        .join_room("", "new-empty", None, Duration::seconds(300))
        .await;
    assert!(
        fresh.is_ok(),
        "fresh empty room survived and accepts a join"
    );
    assert!(matches!(
        store
            .join_room("", "old-empty", None, Duration::seconds(300))
            .await,
        Err(RoomError::RoomNotFound)
    ));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn reap_never_crosses_namespaces() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let now = Utc::now();
    let ttl = Duration::minutes(30);

    // Same room_id in two namespaces: ns "a" is stale, ns "b" is fresh.
    seed(
        &pool,
        "a",
        "shared-id",
        now - Duration::hours(2),
        &[("stale", "tok-a", now - Duration::hours(1))],
    )
    .await;
    seed(
        &pool,
        "b",
        "shared-id",
        now - Duration::minutes(1),
        &[("fresh", "tok-b", now - Duration::minutes(1))],
    )
    .await;

    let stats = store.reap_stale(now, ttl).await;
    assert_eq!(stats.rooms_reaped, 1);
    assert_eq!(stats.participants_reaped, 1);

    // ns "a" room reaped; the identically-named ns "b" room is untouched.
    assert!(matches!(
        store.roster("a", "shared-id", "tok-a").await,
        Err(RoomError::RoomNotFound)
    ));
    assert!(store.roster("b", "shared-id", "tok-b").await.is_ok());
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn reap_on_a_clean_store_is_a_zero_count_no_op() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let now = Utc::now();

    // A fresh room+seat: nothing to reap.
    seed(
        &pool,
        "",
        "room-1",
        now - Duration::minutes(1),
        &[("fresh", "tok", now - Duration::minutes(1))],
    )
    .await;
    let stats = store.reap_stale(now, Duration::minutes(30)).await;
    assert_eq!(stats.participants_reaped, 0);
    assert_eq!(stats.rooms_reaped, 0);
    assert!(store.roster("", "room-1", "tok").await.is_ok());
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn heartbeat_holds_a_seat_across_a_sweep_and_renews_the_advisory_expiry() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let stale = Utc::now() - Duration::hours(1);
    seed(
        &pool,
        "",
        "room-1",
        Utc::now() - Duration::hours(2),
        &[("beating", "tok-a", stale), ("silent", "tok-b", stale)],
    )
    .await;

    let before = Utc::now();
    let renewed = store
        .heartbeat("", "room-1", "beating", "tok-a", Duration::seconds(300))
        .await
        .expect("heartbeat");
    // The renewal honors the supplied TTL, not some other horizon.
    assert!(renewed >= before + Duration::seconds(300) - Duration::microseconds(1));
    assert!(renewed <= Utc::now() + Duration::seconds(300));

    // The renewed expiry is persisted, so another process sees it.
    let persisted: chrono::NaiveDateTime = {
        let mut conn = pool.get().await.expect("conn");
        let row: ExpiryRow = diesel::sql_query(
            "SELECT token_expires_at FROM media_room_participants \
             WHERE namespace = '' AND room_id = 'room-1' AND participant_id = 'beating'",
        )
        .get_result(&mut conn)
        .await
        .expect("read expiry");
        row.token_expires_at
    };
    assert_eq!(persisted, renewed.naive_utc());

    // The heartbeat — not a roster poll — is what saves the seat.
    let stats = store.reap_stale(Utc::now(), Duration::minutes(30)).await;
    assert_eq!(stats.participants_reaped, 1, "only the silent seat reaped");
    assert!(store.roster("", "room-1", "tok-a").await.is_ok());
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn heartbeat_is_fail_closed_with_no_membership_oracle() {
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let now = Utc::now();
    seed(&pool, "tenant-a", "room-1", now, &[("p1", "tok", now)]).await;
    let ttl = Duration::seconds(300);

    for (namespace, room, participant, token, case) in [
        ("tenant-a", "nope", "p1", "tok", "unknown room"),
        ("tenant-a", "room-1", "ghost", "tok", "unknown participant"),
        ("tenant-a", "room-1", "p1", "wrong", "wrong token"),
        ("tenant-b", "room-1", "p1", "tok", "other namespace"),
    ] {
        assert!(
            matches!(
                store
                    .heartbeat(namespace, room, participant, token, ttl)
                    .await,
                Err(RoomError::RoomNotFound)
            ),
            "{case} must be indistinguishable from a missing room"
        );
    }
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn heartbeat_rejects_a_sibling_participants_token() {
    // The token is verified against the named participant, not against any
    // member of the room.
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let now = Utc::now();
    seed(
        &pool,
        "",
        "room-1",
        now,
        &[("p1", "tok-1", now), ("p2", "tok-2", now)],
    )
    .await;

    assert!(matches!(
        store
            .heartbeat("", "room-1", "p2", "tok-1", Duration::seconds(300))
            .await,
        Err(RoomError::RoomNotFound)
    ));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn heartbeat_on_a_seat_reaped_concurrently_reports_it_gone() {
    // The store reads the token, then writes. A reaper (or another process's
    // leave) between the two renews nothing, which must not read as success.
    let (pool, _container) = setup_pool().await;
    let store = DbRoomStore::new(pool.clone(), 6);
    let stale = Utc::now() - Duration::hours(1);
    seed(&pool, "", "room-1", stale, &[("p1", "tok", stale)]).await;

    let stats = store.reap_stale(Utc::now(), Duration::minutes(30)).await;
    assert_eq!(stats.participants_reaped, 1);

    assert!(matches!(
        store
            .heartbeat("", "room-1", "p1", "tok", Duration::seconds(300))
            .await,
        Err(RoomError::RoomNotFound)
    ));
}

/// One `count(*)` column, for asserting row presence straight from SQL.
#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    c: i64,
}

/// Poll `pg_stat_activity` until some backend is blocked on a lock while its
/// query touches `media_rooms` (or time out and fail: without the block, the
/// race below would not actually be exercised and the test could pass
/// vacuously).
async fn wait_for_room_lock_wait(pool: &Pool<AsyncPgConnection>) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let mut conn = pool.get().await.expect("conn");
        let rows: Vec<CountRow> = diesel::sql_query(
            "SELECT count(*) AS c FROM pg_stat_activity \
             WHERE wait_event_type = 'Lock' AND query ILIKE '%media_rooms%'",
        )
        .load::<CountRow>(&mut conn)
        .await
        .expect("pg_stat_activity");
        // NOTE: do not call `rows.first()` here — `diesel_async::RunQueryDsl`
        // is in scope and its `first` (limit-1 query) shadows the slice method
        // at method-probing time.
        let blocked = rows.into_iter().next().map_or(0, |r| r.c) > 0;
        if blocked {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "reaper's phase-2 DELETE never blocked on the room row lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Issue #2407, deterministic: a join whose transaction holds the room row
/// lock *and a new row version* (the `SET created_at = created_at` touch)
/// defeats a concurrent `reap_stale` phase-2 `DELETE`.
///
/// The `DELETE` evaluates the room as empty under its statement snapshot,
/// then blocks on the join's row lock. When the join commits, the delete
/// wakes to a new row version, so READ COMMITTED's `EvalPlanQual` recheck
/// re-evaluates the `NOT EXISTS` empty-room predicate against a fresh
/// snapshot, sees the committed participant, and skips the room. The room
/// and the brand-new participant both survive.
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn reap_phase2_delete_rechecks_emptiness_after_a_concurrent_join_commits() {
    let (pool, _container) = setup_pool().await;
    let now = Utc::now();
    let ttl = Duration::minutes(30);

    // An old, empty room: reap-eligible on the reaper's own snapshot.
    seed(&pool, "", "race-room", now - Duration::hours(2), &[]).await;

    // T1: BEGIN, then exactly the touch join_room performs — takes the room
    // row lock and mints a new row version, holding both to commit.
    let mut tx = pool.get().await.expect("tx conn");
    diesel::sql_query("BEGIN")
        .execute(&mut tx)
        .await
        .expect("begin");
    let touched: usize = diesel::sql_query(
        "UPDATE media_rooms SET created_at = created_at \
         WHERE namespace = '' AND room_id = 'race-room'",
    )
    .execute(&mut tx)
    .await
    .expect("touch");
    assert_eq!(touched, 1, "touch must hit the seeded room");

    // T2: the real reaper. Its phase-2 DELETE evaluates the room as empty,
    // then blocks on T1's row lock.
    let store = DbRoomStore::new(pool.clone(), 6);
    let reap = tokio::spawn(async move { store.reap_stale(now, ttl).await });
    wait_for_room_lock_wait(&pool).await;

    // T1: the join's participant INSERT lands, then commit.
    let joined_at = now.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f");
    diesel::sql_query(format!(
        "INSERT INTO media_room_participants \
         (namespace, room_id, participant_id, display_name, token, \
          joined_at, token_expires_at, last_seen_at) \
         VALUES ('', 'race-room', 'p1', NULL, 'tok', \
                 '{joined_at}', '{joined_at}', '{joined_at}')"
    ))
    .execute(&mut tx)
    .await
    .expect("insert participant");
    diesel::sql_query("COMMIT")
        .execute(&mut tx)
        .await
        .expect("commit");

    let stats = reap.await.expect("reap task");
    assert_eq!(
        stats.rooms_reaped, 0,
        "reaper must skip the room the concurrent join claimed"
    );

    // The room and the brand-new participant both survive.
    let mut conn = pool.get().await.expect("conn");
    let rooms: Vec<CountRow> =
        diesel::sql_query("SELECT count(*) AS c FROM media_rooms WHERE room_id = 'race-room'")
            .load::<CountRow>(&mut conn)
            .await
            .expect("count rooms");
    assert_eq!(rooms[0].c, 1, "room must survive the concurrent reap");
    let seat_rows: Vec<CountRow> = diesel::sql_query(
        "SELECT count(*) AS c FROM media_room_participants WHERE participant_id = 'p1'",
    )
    .load::<CountRow>(&mut conn)
    .await
    .expect("count participants");
    assert_eq!(
        seat_rows[0].c, 1,
        "the join's participant must survive the reap"
    );
}

/// Companion to the test above: documents *why* the touch is needed. A
/// lock-only `SELECT ... FOR NO KEY UPDATE` (the naive fix) does NOT stop
/// the reaper — a lock-only `xmax` triggers no `EvalPlanQual` recheck, so the
/// `DELETE` proceeds on its stale snapshot and the room (plus the new
/// participant, via the cascade) is gone. If this ever starts failing, the
/// premise of the touch has changed and the fix deserves a re-think.
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn a_lock_without_a_touch_does_not_stop_the_reaper() {
    let (pool, _container) = setup_pool().await;
    let now = Utc::now();
    let ttl = Duration::minutes(30);

    seed(&pool, "", "race-room", now - Duration::hours(2), &[]).await;

    // T1: BEGIN, lock the room row *without* touching it, hold to commit.
    let mut tx = pool.get().await.expect("tx conn");
    diesel::sql_query("BEGIN")
        .execute(&mut tx)
        .await
        .expect("begin");
    diesel::sql_query(
        "SELECT namespace FROM media_rooms \
         WHERE namespace = '' AND room_id = 'race-room' FOR NO KEY UPDATE",
    )
    .execute(&mut tx)
    .await
    .expect("lock");

    let store = DbRoomStore::new(pool.clone(), 6);
    let reap = tokio::spawn(async move { store.reap_stale(now, ttl).await });
    wait_for_room_lock_wait(&pool).await;

    let joined_at = now.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f");
    diesel::sql_query(format!(
        "INSERT INTO media_room_participants \
         (namespace, room_id, participant_id, display_name, token, \
          joined_at, token_expires_at, last_seen_at) \
         VALUES ('', 'race-room', 'p1', NULL, 'tok', \
                 '{joined_at}', '{joined_at}', '{joined_at}')"
    ))
    .execute(&mut tx)
    .await
    .expect("insert participant");
    diesel::sql_query("COMMIT")
        .execute(&mut tx)
        .await
        .expect("commit");

    let stats = reap.await.expect("reap task");
    // The naive lock-only shape loses: the DELETE saw no new row version, so
    // no EPQ recheck ran and the stale snapshot deleted the room.
    assert_eq!(stats.rooms_reaped, 1, "lock-only must NOT save the room");

    let mut conn = pool.get().await.expect("conn");
    let rooms: Vec<CountRow> =
        diesel::sql_query("SELECT count(*) AS c FROM media_rooms WHERE room_id = 'race-room'")
            .load::<CountRow>(&mut conn)
            .await
            .expect("count rooms");
    assert_eq!(rooms[0].c, 0, "room is gone under the lock-only shape");
}

/// Issue #2407, end to end: hammer real `join_room` calls against a
/// hard-sweeping reaper and assert the strict invariant — every join that
/// reported `Ok` still has its participant row and its room row afterwards.
/// A join that loses the race must fail loud (`RoomNotFound` / `RoomFull`),
/// never a phantom success whose seat silently vanishes.
///
/// Strict on fixed code (no flaky failures: a later reap can never delete a
/// room that holds fresh participants); probabilistic detection on unfixed
/// code (the barrier + hammer make the snapshot-to-delete window very likely
/// to catch at least one join).
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn concurrent_joins_racing_the_reaper_never_silently_vanish() {
    const JOINERS: usize = 8;
    const JOINS_EACH: usize = 10;
    const REAP_TICKS: usize = 40;

    let (pool, _container) = setup_pool().await;
    let now = Utc::now();
    let ttl = Duration::minutes(30);

    // One old, empty room: reap-eligible and joinable.
    seed(&pool, "", "hammer-room", now - Duration::hours(2), &[]).await;

    let barrier = Arc::new(tokio::sync::Barrier::new(JOINERS + 2));
    let mut handles = Vec::with_capacity(JOINERS);
    for _ in 0..JOINERS {
        let pool = pool.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let store = DbRoomStore::new(pool, 64);
            let mut oks = Vec::new();
            for _ in 0..JOINS_EACH {
                match store
                    .join_room("", "hammer-room", None, Duration::seconds(300))
                    .await
                {
                    Ok(rec) => oks.push(rec.participant_id),
                    // Loud losses: the reaper won (room gone) or the cap hit.
                    Err(RoomError::RoomNotFound | RoomError::RoomFull { .. }) => {}
                    Err(other) => panic!("unexpected join error: {other:?}"),
                }
            }
            oks
        }));
    }
    let reaper_pool = pool.clone();
    let reaper_barrier = Arc::clone(&barrier);
    let reaper = tokio::spawn(async move {
        reaper_barrier.wait().await;
        let store = DbRoomStore::new(reaper_pool, 64);
        for _ in 0..REAP_TICKS {
            store.reap_stale(now, ttl).await;
        }
    });

    barrier.wait().await;
    let mut ok_ids = Vec::new();
    for handle in handles {
        ok_ids.extend(handle.await.expect("join task panicked"));
    }
    reaper.await.expect("reaper task panicked");

    assert!(
        !ok_ids.is_empty(),
        "expected at least one join to succeed under the hammer"
    );
    let mut conn = pool.get().await.expect("conn");
    for pid in &ok_ids {
        let rows: Vec<CountRow> = diesel::sql_query(format!(
            "SELECT count(*) AS c FROM media_room_participants \
             WHERE participant_id = '{pid}'"
        ))
        .load::<CountRow>(&mut conn)
        .await
        .expect("count participant");
        assert_eq!(
            rows[0].c, 1,
            "join {pid} reported Ok but its seat silently vanished"
        );
    }
    let rooms: Vec<CountRow> =
        diesel::sql_query("SELECT count(*) AS c FROM media_rooms WHERE room_id = 'hammer-room'")
            .load::<CountRow>(&mut conn)
            .await
            .expect("count rooms");
    assert_eq!(
        rooms[0].c, 1,
        "room vanished out from under successful joins"
    );
}
