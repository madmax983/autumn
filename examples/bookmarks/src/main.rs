// Bookmarks — an Autumn v0.2 example showcasing all new features:
//
//   Profiles        → autumn.toml + autumn-dev.toml (dev auto-detected)
//   Validation      → Valid<Json<NewBookmark>> with #[validate] rules
//   Scheduled tasks → #[scheduled(every = "1h")] link health checker
//   Actuator        → /actuator/health, /actuator/info, /actuator/env
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

use autumn_web::prelude::*;
use diesel::Connection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    // Run pending migrations on startup
    let config = autumn_web::config::AutumnConfig::load().expect("load config");
    if let Some(url) = &config.database.url {
        let mut conn =
            diesel::PgConnection::establish(url).expect("connect to database for migrations");
        conn.run_pending_migrations(MIGRATIONS)
            .expect("run migrations");
    }

    // ── v0.2: .tasks() registers scheduled background tasks ─────
    autumn_web::app()
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
