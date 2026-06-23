use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;
use saas::routes;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .migrations(MIGRATIONS)
        .routes(routes![
            saas::index,
            routes::auth::signup_form,
            routes::auth::signup,
            routes::auth::login_form,
            routes::auth::login,
            routes::auth::logout,
            routes::dashboard::dashboard,
            routes::dashboard::create_project,
        ])
        .run()
        .await;
}
