pub mod hooks;
pub mod jobs;
pub mod live_bus;
pub mod live_events;
pub mod models;
pub mod policies;
pub mod repositories;
pub mod routes;
pub mod schema;
pub mod slugify;
pub mod tasks;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use autumn_web::config::{AutumnConfig, MockEnv};
    use diesel_migrations::{EmbeddedMigrations, embed_migrations};

    use crate::live_bus::{LiveFeedBusConfig, LiveFeedBusKind};

    const _REDDIT_MIGRATIONS: EmbeddedMigrations = embed_migrations!();

    const MIGRATION_SQL: &str = include_str!("../migrations/20260419000000_create_reddit/up.sql");
    const MIGRATION_DOWN_SQL: &str =
        include_str!("../migrations/20260419000000_create_reddit/down.sql");

    #[test]
    fn migration_uses_bigserial_ids() {
        assert!(
            MIGRATION_SQL.contains("BIGSERIAL PRIMARY KEY"),
            "All IDs must be 64-bit to match the Int8/i64 application schema",
        );
    }

    #[test]
    fn migration_creates_all_tables() {
        for table in &[
            "users",
            "subreddits",
            "posts",
            "comments",
            "votes",
            "live_feed_events",
        ] {
            assert!(
                MIGRATION_SQL.contains(&format!("CREATE TABLE {table}")),
                "Migration must create the '{table}' table",
            );
        }
    }

    #[test]
    fn migration_down_drops_live_feed_events() {
        assert!(
            MIGRATION_DOWN_SQL.contains("DROP TABLE IF EXISTS live_feed_events"),
            "Rollback migration must drop the live_feed_events table added by up.sql",
        );
    }

    #[test]
    fn dev_profile_enables_csrf_for_forms() {
        let env = MockEnv::new()
            .with("AUTUMN_PROFILE", "dev")
            .with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));

        let config =
            AutumnConfig::load_with_env(&env).expect("reddit-clone dev config should load");

        assert!(
            config.security.csrf.enabled,
            "reddit-clone extracts `CsrfToken`, so its dev profile must enable CSRF",
        );
    }

    #[test]
    fn redis_profile_uses_redis_jobs_backend() {
        let env = MockEnv::new()
            .with("AUTUMN_PROFILE", "redis")
            .with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));

        let config =
            AutumnConfig::load_with_env(&env).expect("reddit-clone redis config should load");

        assert_eq!(config.jobs.backend, "redis");
        assert_eq!(config.jobs.workers, 2);
        assert_eq!(
            config.jobs.redis.url.as_deref(),
            Some("redis://127.0.0.1:6379/")
        );
    }

    #[test]
    fn redis_profile_declares_redis_live_feed_bus() {
        let env = MockEnv::new()
            .with("AUTUMN_PROFILE", "redis")
            .with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));
        let config =
            LiveFeedBusConfig::load_with_env(&env).expect("redis live-feed bus config should load");

        assert_eq!(
            config.kind,
            LiveFeedBusKind::RedisPubSub,
            "redis profile should use an external live-feed bus instead of Postgres notify",
        );
        assert_eq!(config.redis_url.as_deref(), Some("redis://127.0.0.1:6379/"),);
    }

    #[test]
    fn default_live_feed_bus_uses_postgres_notify() {
        let env = MockEnv::new().with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));
        let config = LiveFeedBusConfig::load_with_env(&env)
            .expect("default live-feed bus config should load");

        assert_eq!(config.kind, LiveFeedBusKind::PostgresNotify);
        assert_eq!(config.redis_url, None);
        assert_eq!(config.channel, "reddit_live_feed");
    }

    #[test]
    fn reddit_background_jobs_register_request_side_effects() {
        use crate::jobs::{PostPublicationJob, UserOnboardingJob};

        let jobs = crate::jobs::registered_jobs();
        let by_name = jobs
            .iter()
            .map(|job| {
                (
                    job.name.as_str(),
                    (job.max_attempts, job.initial_backoff_ms),
                )
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(jobs.len(), 2);
        assert!(by_name.contains(&("user_onboarding", (5, 500))));
        assert!(by_name.contains(&("post_publication", (5, 500))));

        // NAME constants must match the registered job names so enqueue_on_conn callers
        // don't need to duplicate the string.
        assert_eq!(UserOnboardingJob::NAME, "user_onboarding");
        assert_eq!(PostPublicationJob::NAME, "post_publication");
    }

    #[test]
    fn default_profile_uses_postgres_jobs_backend() {
        let env = MockEnv::new().with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));
        let config = AutumnConfig::load_with_env(&env).expect("default config should load");
        assert_eq!(
            config.jobs.backend, "postgres",
            "autumn.toml must set jobs.backend = \"postgres\" as the default"
        );
    }
}
