mod models;
mod routes;
mod schema;

use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::{jobs, routes, static_routes};

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
            routes::api::enqueue_publish_webhook,
        ])
        .jobs(jobs![routes::api::publish_webhook])
        .static_routes(static_routes![routes::about::about,])
        .run()
        .await;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    const MIGRATION_SQL: &str = include_str!("../migrations/00000000000000_create_posts/up.sql");

    #[test]
    fn migration_uses_bigserial_ids() {
        assert!(
            MIGRATION_SQL.contains("id BIGSERIAL PRIMARY KEY"),
            "post IDs must be 64-bit to match the Int8/i64 application schema",
        );
    }

    #[test]
    fn upgrade_migration_widens_existing_ids() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations/00000000000001_widen_post_ids_to_bigint/up.sql");
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing upgrade migration at {}: {err}", path.display()));

        assert!(
            sql.contains("ALTER TABLE posts ALTER COLUMN id TYPE BIGINT"),
            "post upgrade migration must widen existing IDs to BIGINT",
        );
        assert!(
            sql.contains("ALTER SEQUENCE posts_id_seq AS BIGINT"),
            "post upgrade migration must widen the backing sequence to BIGINT",
        );
    }
}
