pub mod harvest_runtime;
pub mod hooks;
pub mod live_bus;
pub mod live_events;
pub mod models;
pub mod policies;
pub mod repositories;
pub mod routes;
pub mod schema;
pub mod slugify;
pub mod tasks;
pub mod workflows;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use autumn_harvest_plugin::{HarvestMode, HarvestRuntimeConfig};
    use autumn_web::config::{AutumnConfig, MockEnv};
    use diesel::migration::MigrationSource;
    use diesel::pg::Pg;
    use diesel_migrations::{EmbeddedMigrations, embed_migrations};

    use crate::live_bus::{LiveFeedBusConfig, LiveFeedBusKind};

    const REDDIT_MIGRATIONS: EmbeddedMigrations = embed_migrations!();

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
    fn app_and_harvest_migration_versions_do_not_collide() {
        // Pulls migration version prefixes from the actual EmbeddedMigrations
        // that get applied at runtime — no filesystem reads into sibling
        // crates (which broke when autumn-harvest moved to its own repo).
        //
        // TODO: also check autumn-harvest-plugin's outbox migrations once the
        // plugin exposes them as a `pub const`. They're currently private to
        // the plugin crate, so a downstream consumer can't see them.
        let app = migration_versions(&REDDIT_MIGRATIONS);
        let harvest = migration_versions(&autumn_harvest::MIGRATIONS);

        let collisions: Vec<_> = app.intersection(&harvest).cloned().collect();
        assert!(
            collisions.is_empty(),
            "reddit-clone and Harvest migrations must not share Diesel version prefixes: {collisions:?}",
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
    fn split_web_profile_disables_local_harvest_runtime_ownership() {
        let env = MockEnv::new()
            .with("AUTUMN_PROFILE", "split-web")
            .with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));

        let config = HarvestRuntimeConfig::load_with_env(&env)
            .expect("split-web harvest config should load");

        assert_eq!(config.mode, HarvestMode::Split);
        assert!(!config.worker_enabled);
        assert!(!config.scheduler_enabled);
        assert_eq!(
            config.database.url.as_deref(),
            Some("postgres://autumn:autumn@localhost:5432/reddit_harvest")
        );
    }

    #[test]
    fn split_web_profile_declares_redis_live_feed_bus() {
        let env = MockEnv::new()
            .with("AUTUMN_PROFILE", "split-web")
            .with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));
        let config = LiveFeedBusConfig::load_with_env(&env)
            .expect("split-web live-feed bus config should load");

        assert_eq!(
            config.kind,
            LiveFeedBusKind::RedisPubSub,
            "split-web should use an external live-feed bus instead of Postgres notify",
        );
        assert_eq!(config.redis_url.as_deref(), Some("redis://127.0.0.1:6379/"),);
    }

    #[test]
    fn split_runner_profile_enables_local_harvest_runtime_ownership() {
        let env = MockEnv::new()
            .with("AUTUMN_PROFILE", "split-runner")
            .with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));

        let config = HarvestRuntimeConfig::load_with_env(&env)
            .expect("split-runner harvest config should load");

        assert_eq!(config.mode, HarvestMode::Split);
        assert!(config.worker_enabled);
        assert!(config.scheduler_enabled);
        assert_eq!(
            config.database.url.as_deref(),
            Some("postgres://autumn:autumn@localhost:5432/reddit_harvest")
        );
    }

    #[test]
    fn split_runner_profile_declares_redis_live_feed_bus() {
        let env = MockEnv::new()
            .with("AUTUMN_PROFILE", "split-runner")
            .with("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));
        let config = LiveFeedBusConfig::load_with_env(&env)
            .expect("split-runner live-feed bus config should load");

        assert_eq!(
            config.kind,
            LiveFeedBusKind::RedisPubSub,
            "split-runner should publish live-feed wakeups through Redis",
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

    fn migration_versions(source: &EmbeddedMigrations) -> BTreeSet<String> {
        MigrationSource::<Pg>::migrations(source)
            .expect("EmbeddedMigrations should yield its migrations")
            .iter()
            .map(|migration| {
                migration
                    .name()
                    .to_string()
                    .split('_')
                    .next()
                    .expect("migration name should have a version prefix")
                    .to_owned()
            })
            .collect()
    }
}
