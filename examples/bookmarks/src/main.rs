// Bookmarks — an Autumn example showcasing the post-v0.1.0 feature set:
//
//   Profiles        ? autumn.toml + autumn-dev.toml (dev auto-detected)
//   CRUD API        ? #[repository(api = "/api/bookmarks")] generates REST handlers
//   Scheduled tasks ? #[scheduled(every = "1h")] link health checker
//   Actuator        ? /actuator/health, /actuator/info, /actuator/env
//
// Run with:  cargo run -p bookmarks
// API test:  curl -X POST http://localhost:3000/api/bookmarks \
//              -H 'Content-Type: application/json' \
//              -d '{"url":"https://rust-lang.org","title":"Rust","tag":"lang"}'

mod models;
mod repositories;
mod routes;
mod schema;
mod tasks;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    // -- v0.2: .tasks() registers scheduled background tasks -----
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::bookmarks::list,
            routes::bookmarks::by_tag,
            routes::bookmarks::new_form,
            routes::bookmarks::create,
            repositories::bookmark_api_list,
            repositories::bookmark_api_get,
            repositories::bookmark_api_create,
            repositories::bookmark_api_update,
            repositories::bookmark_api_delete,
        ])
        .tasks(tasks![tasks::check_links])
        .run()
        .await;
}
