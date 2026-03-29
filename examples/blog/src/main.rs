mod models;
mod routes;
mod schema;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::{routes, static_routes};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
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
        .static_routes(static_routes![routes::about::about,])
        .run()
        .await;
}
