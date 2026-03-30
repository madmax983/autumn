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

#[cfg(test)]
mod tests {
    use std::path::Path;

    const MIGRATION_SQL: &str = include_str!("../migrations/00000000000000_create_todos/up.sql");

    #[test]
    fn migration_uses_bigserial_ids() {
        assert!(
            MIGRATION_SQL.contains("id BIGSERIAL PRIMARY KEY"),
            "todo IDs must be 64-bit to match the Int8/i64 application schema",
        );
    }

    #[test]
    fn upgrade_migration_widens_existing_ids() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations/00000000000001_widen_todo_ids_to_bigint/up.sql");
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing upgrade migration at {}: {err}", path.display()));

        assert!(
            sql.contains("ALTER TABLE todos ALTER COLUMN id TYPE BIGINT"),
            "todo upgrade migration must widen existing IDs to BIGINT",
        );
        assert!(
            sql.contains("ALTER SEQUENCE todos_id_seq AS BIGINT"),
            "todo upgrade migration must widen the backing sequence to BIGINT",
        );
    }
}
