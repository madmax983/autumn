#![allow(missing_docs)]
mod hooks;
mod models;
mod repositories;
mod routes;
mod schema;
mod slugify;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::pages::list,
            routes::pages::show,
            routes::pages::new_form,
            routes::pages::create,
            routes::pages::edit_form,
            routes::pages::update,
            routes::pages::history,
            repositories::page_api_list,
            repositories::page_api_get,
            repositories::page_api_create,
            repositories::page_api_update,
            repositories::page_api_delete,
        ])
        .run()
        .await;
}
