mod models;
mod routes;
mod schema;

use autumn_web::routes;
use diesel::Connection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    // Run pending database migrations before starting the server.
    let config = autumn_web::config::AutumnConfig::load().expect("load config");
    if let Some(url) = &config.database.url {
        let mut conn =
            diesel::PgConnection::establish(url).expect("connect to database for migrations");
        conn.run_pending_migrations(MIGRATIONS)
            .expect("run migrations");
    }

    autumn_web::app()
        .routes(routes![
            // Public routes
            routes::about::about, // #[static_get] — pre-rendered
            routes::posts::index,
            routes::posts::show,
            // Admin routes
            routes::posts::admin_list,
            routes::posts::new_form,
            routes::posts::create,
            routes::posts::edit_form,
            routes::posts::update,
            routes::posts::delete_post,
            // JSON API
            routes::api::list_json,
            routes::api::create_json,
        ])
        .run()
        .await;
}
