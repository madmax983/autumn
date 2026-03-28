mod hooks;
mod models;
mod repositories;
mod routes;
mod schema;
mod slugify;

use autumn_web::prelude::*;
use diesel::Connection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    let config = autumn_web::config::AutumnConfig::load().expect("load config");
    if let Some(url) = &config.database.url {
        let mut conn =
            diesel::PgConnection::establish(url).expect("connect to database for migrations");
        conn.run_pending_migrations(MIGRATIONS)
            .expect("run migrations");
    }

    autumn_web::app()
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
