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
//   Durable Workflows   -> autumn-harvest patterns (see workflows.rs)
//   Profiles            -> autumn.toml + autumn-dev.toml dev overrides
//   Actuator            -> /health, /actuator/health, /actuator/info, /actuator/tasks
//   HTML stack          -> Maud templates, htmx interactivity, Tailwind CSS
//
// Run with:   cargo run -p reddit-clone
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

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;

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
        .run()
        .await;
}

#[cfg(test)]
mod tests {
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
}
