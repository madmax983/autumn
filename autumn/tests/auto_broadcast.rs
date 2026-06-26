//! Integration tests for declarative auto-broadcasting.
//!
//! Run with:
//!
//!     cargo test --test auto_broadcast --features "ws,db"

#[cfg(all(feature = "db", feature = "ws"))]
mod tests {
    use autumn_web::prelude::*;
    use autumn_web::test::TestDb;
    use diesel_async::RunQueryDsl;

    // ── Schema ─────────────────────────────────────────────────

    diesel::table! {
        broadcast_posts (id) {
            id -> Int8,
            title -> Text,
        }
    }

    #[autumn_web::model(table = "broadcast_posts")]
    pub struct BroadcastPost {
        #[id]
        pub id: i64,
        pub title: String,
    }

    // A model with a custom render function.
    fn render_custom_post(post: &BroadcastPost) -> maud::Markup {
        maud::html! {
            div id=(format!("custom-post-{}", post.id)) {
                h2 { (post.title) }
            }
        }
    }

    // 1. Basic auto-broadcasting (defaults to topic = "broadcast_posts", container = "broadcast_posts-list")
    #[autumn_web::repository(BroadcastPost, table = "broadcast_posts", broadcasts = true)]
    pub trait BasicPostRepository {}

    // 2. Custom broadcasting (custom topic with field interpolation, custom container, custom render)
    #[autumn_web::repository(
        BroadcastPost,
        table = "broadcast_posts",
        broadcasts = true,
        topic = "post_topic:{title}",
        render = render_custom_post,
        container = "custom-posts-list"
    )]
    pub trait CustomPostRepository {}

    // ── Setup ──────────────────────────────────────────────────

    async fn setup_db(db: &TestDb) {
        let mut conn = db.pool().get().await.expect("db connection");
        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS broadcast_posts (
                id BIGSERIAL PRIMARY KEY,
                title TEXT NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await
        .expect("create broadcast_posts table");

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS autumn_repository_commit_hooks (
                id TEXT PRIMARY KEY,
                handler_key TEXT NOT NULL,
                hook_name TEXT NOT NULL,
                context JSONB NOT NULL,
                record JSONB NOT NULL,
                status TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                max_attempts INTEGER NOT NULL,
                initial_backoff_ms BIGINT NOT NULL,
                enqueued_at TIMESTAMP NOT NULL,
                run_at TIMESTAMP NOT NULL,
                claimed_by TEXT,
                claimed_at TIMESTAMP,
                started_at TIMESTAMP,
                finished_at TIMESTAMP,
                last_error TEXT
            )",
        )
        .execute(&mut *conn)
        .await
        .expect("create autumn_repository_commit_hooks table");
    }

    // ── Tests ──────────────────────────────────────────────────

    #[tokio::test]
    #[ignore = "requires Docker (testcontainers)"]
    async fn test_auto_broadcast_lifecycle() {
        let db = TestDb::shared().await;
        setup_db(db).await;

        let state = AppState::detached();
        let channels = state.channels().clone();
        // Register channels globally in the worker
        autumn_web::__private::set_global_channels(channels.clone());

        // Subscribe to basic and custom topics
        let mut basic_sub = channels.subscribe("broadcast_posts");
        let mut custom_sub = channels.subscribe("post_topic:hello");

        let repo = PgBasicPostRepository::with_pool_untracked(db.pool().clone());
        let custom_repo = PgCustomPostRepository::with_pool_untracked(db.pool().clone());

        // Start the background commit hook worker
        let shutdown = tokio_util::sync::CancellationToken::new();
        autumn_web::__private::start_repository_commit_hook_worker(
            db.pool().clone(),
            Some(channels.clone()),
            shutdown.child_token(),
        );

        // 1. Create on basic repository
        let new_post = repo
            .save(&NewBroadcastPost {
                title: "hello".to_owned(),
            })
            .await
            .expect("save post");

        // Wait for the background worker to drain the hook and publish
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), basic_sub.recv())
            .await
            .expect("timeout waiting for basic create broadcast")
            .expect("recv error");
        let html_content = msg.into_string();
        assert!(html_content.contains("hx-swap-oob=\"beforeend:#broadcast_posts-list\""));
        assert!(html_content.contains(&format!("broadcast_post-{}", new_post.id)));

        // 2. Create on custom repository
        let custom_post = custom_repo
            .save(&NewBroadcastPost {
                title: "hello".to_owned(),
            })
            .await
            .expect("save custom post");

        // Wait for the background worker to drain and publish on custom topic (interpolated post_topic:hello)
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), custom_sub.recv())
            .await
            .expect("timeout waiting for custom create broadcast")
            .expect("recv error");
        let html_content = msg.into_string();
        assert!(html_content.contains("hx-swap-oob=\"beforeend:#custom-posts-list\""));
        assert!(html_content.contains(&format!("id=\"custom-post-{}", custom_post.id)));
        assert!(html_content.contains("hello"));

        shutdown.cancel();
    }
}
