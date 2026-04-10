pub mod harvest_runtime;
pub mod hooks;
pub mod live_bus;
pub mod live_events;
pub mod models;
pub mod repositories;
pub mod routes;
pub mod schema;
pub mod slugify;
pub mod tasks;
pub mod workflows;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use autumn_web::config::{AutumnConfig, MockEnv};
    use autumn_web_harvest::{HarvestMode, HarvestRuntimeConfig};

    use crate::live_bus::{LiveFeedBusConfig, LiveFeedBusKind};

    const MIGRATION_SQL: &str = include_str!("../migrations/00000000000000_create_reddit/up.sql");

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
    fn app_and_harvest_migration_versions_do_not_collide() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let app_migrations = collect_migration_versions(&manifest_dir.join("migrations"));
        let harvest_adapter_migrations = collect_migration_versions(
            &manifest_dir.join("../../autumn-harvest/autumn-web-harvest/migrations"),
        );
        let harvest_migrations = collect_migration_versions(
            &manifest_dir.join("../../autumn-harvest/autumn-harvest/migrations"),
        );

        let collisions: Vec<_> = app_migrations
            .intersection(&harvest_adapter_migrations)
            .cloned()
            .collect();

        assert!(
            collisions.is_empty(),
            "reddit-clone and autumn-web-harvest migrations must not share Diesel version prefixes: {collisions:?}",
        );

        let collisions: Vec<_> = app_migrations
            .intersection(&harvest_migrations)
            .cloned()
            .collect();

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

    fn collect_migration_versions(dir: &Path) -> BTreeSet<String> {
        fs::read_dir(dir)
            .unwrap_or_else(|error| {
                panic!("failed to read migrations at {}: {error}", dir.display())
            })
            .map(|entry| entry.expect("migration directory entry should be readable"))
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_dir())
                    .map(|_| entry)
            })
            .map(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .split('_')
                    .next()
                    .expect("migration directory should have a version prefix")
                    .to_owned()
            })
            .collect()
    }
}
