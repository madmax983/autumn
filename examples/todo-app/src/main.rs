mod models;
mod routes;
mod schema;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::routes;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::todos::index,
            routes::todos::list,
            routes::todos::detail,
            routes::todos::create,
            routes::todos::toggle,
            routes::todos::delete_todo,
            routes::api::list_json,
            routes::api::create_json,
        ])
        .run()
        .await;
}
