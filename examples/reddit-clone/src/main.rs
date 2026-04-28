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
//   WebSockets          -> #[ws] live feed with Channels + durable app-db relay + pluggable bus
//   Durable Workflows   -> autumn-harvest onboarding + post-publication workflows + management API
//   Profiles            -> autumn.toml + autumn-dev.toml dev overrides
//   Actuator            -> /health, /actuator/health, /actuator/info, /actuator/tasks
//   HTML stack          -> Maud templates, htmx interactivity, Tailwind CSS
//
// Run with:   cargo run -p reddit-clone   (first dev boot applies reddit + Harvest migrations;
//                                          startup also drains pending Harvest outbox rows and
//                                          starts the durable live-feed relay)
// Front page: http://localhost:3000
// WebSocket:  ws://localhost:3000/ws/feed
// API test:   curl http://localhost:3000/api/posts
//             curl http://localhost:3000/api/subreddits

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;
use reddit_clone::{harvest_runtime, live_events, repositories, routes, tasks};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::posts::front_page,
            routes::about::about,
            routes::auth::register_form,
            routes::auth::register,
            routes::auth::login_form,
            routes::auth::login,
            routes::auth::logout,
            routes::auth::profile,
            routes::avatars::avatar_form,
            routes::avatars::upload_avatar,
            routes::subreddits::list,
            routes::subreddits::create_form,
            routes::subreddits::create,
            routes::subreddits::show,
            routes::posts::submit_form,
            routes::posts::submit_to_sub_form,
            routes::posts::submit,
            routes::posts::show,
            routes::posts::edit_form,
            routes::posts::update,
            routes::posts::delete_post,
            routes::comments::create,
            routes::comments::list_comments,
            routes::votes::upvote,
            routes::votes::downvote,
            routes::live::live_feed_health,
            routes::live::live_feed,
            routes::live::subreddit_feed,
            repositories::subreddit_api_list,
            repositories::subreddit_api_get,
            repositories::post_api_list,
            repositories::post_api_get,
        ])
        .static_routes(static_routes![routes::about::about])
        .tasks(tasks![
            tasks::recalculate_hot_ranks,
            tasks::prune_live_feed_events
        ])
        .plugins((
            harvest_runtime::harvest_plugin(),
            live_events::LiveFeedPlugin::new(),
        ))
        .run()
        .await;
}
