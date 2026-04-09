// Reddit Clone — an Autumn example showcasing the full feature set:
//
//   Route macros        -> #[get], #[post], #[delete], routes![], #[autumn_web::main]
//   Hybrid rendering    -> #[static_get] pre-rendered about page
//   Database            -> Diesel async Postgres, Db extractor, embedded migrations
//   Model macro         -> #[autumn_web::model] with #[id], #[indexed], #[validate], #[default]
//   Repository macro    -> #[autumn_web::repository] with derived queries & REST API generation
//   Mutation hooks      -> before_create / before_update lifecycle hooks
//   Authentication      -> Session cookies, bcrypt hashing, session.rotate_id()
//   Authorization       -> #[secured] macro for route protection
//   CSRF protection     -> CsrfToken extractor in forms
//   Validation          -> #[validate(length(min, max))] on model fields
//   Scheduled tasks     -> #[scheduled(every = "15m")] hot-rank recalculator
//   WebSockets          -> #[ws] live feed with Channels pub/sub
//   Durable Workflows   -> autumn-harvest onboarding + post-publication workflows + management API
//   Profiles            -> autumn.toml + autumn-dev.toml dev overrides
//   Actuator            -> /health, /actuator/health, /actuator/info, /actuator/tasks
//   HTML stack          -> Maud templates, htmx interactivity, Tailwind CSS
//
// Run with:   cargo run -p reddit-clone   (first dev boot applies reddit + Harvest migrations)
// Front page: http://localhost:3000
// WebSocket:  ws://localhost:3000/ws/feed
// API test:   curl http://localhost:3000/api/posts
//             curl http://localhost:3000/api/subreddits

mod hooks;
mod models;
mod repositories;
mod routes;
mod schema;
mod slugify;
mod tasks;
mod workflows;

use autumn_harvest::prelude::WorkerConfig;
use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;
use autumn_web_harvest::prelude::HarvestExt;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            // ── Front page ─────────────────────────────
            routes::posts::front_page,
            // ── Static page (pre-rendered) ─────────────
            routes::about::about,
            // ── Auth ───────────────────────────────────
            routes::auth::register_form,
            routes::auth::register,
            routes::auth::login_form,
            routes::auth::login,
            routes::auth::logout,
            routes::auth::profile,
            // ── Subreddits ─────────────────────────────
            routes::subreddits::list,
            routes::subreddits::create_form,
            routes::subreddits::create,
            routes::subreddits::show,
            // ── Posts ──────────────────────────────────
            routes::posts::submit_form,
            routes::posts::submit_to_sub_form,
            routes::posts::submit,
            routes::posts::show,
            routes::posts::edit_form,
            routes::posts::update,
            routes::posts::delete_post,
            // ── Comments ───────────────────────────────
            routes::comments::create,
            routes::comments::list_comments,
            // ── Votes (htmx) ──────────────────────────
            routes::votes::upvote,
            routes::votes::downvote,
            // ── WebSocket live feeds ───────────────────
            routes::live::live_feed,
            routes::live::subreddit_feed,
            // ── Generated REST API (read-only) ────────
            repositories::subreddit_api_list,
            repositories::subreddit_api_get,
            repositories::post_api_list,
            repositories::post_api_get,
        ])
        .static_routes(static_routes![routes::about::about])
        .tasks(tasks![tasks::recalculate_hot_ranks])
        .workflows(workflows::registered_workflows())
        .activities(workflows::registered_activities())
        .worker(WorkerConfig {
            max_concurrent_workflows: 4,
            max_concurrent_activities: 8,
            ..WorkerConfig::default()
        })
        .harvest_api("/api/harvest")
        .run()
        .await;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use autumn_web::config::{AutumnConfig, MockEnv};

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
        for table in &["users", "subreddits", "posts", "comments", "votes"] {
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
        let harvest_migrations = collect_migration_versions(
            &manifest_dir.join("../../autumn-harvest/autumn-harvest/migrations"),
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
