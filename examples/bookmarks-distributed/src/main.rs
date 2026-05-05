// Bookmarks Distributed - sibling example scaffolded from bookmarks so the
// future distributed retrofit has a clean, separate home:
//
//   Profiles        -> autumn.toml + autumn-dev.toml (dev auto-detected)
//   CRUD API        -> explicit /api/bookmarks handlers in repositories.rs
//   Scheduled tasks -> #[scheduled(every = "1h")] link health checker
//   Actuator        -> /actuator/health, /actuator/info, /actuator/env
//
// Run with:  cargo run -p bookmarks-distributed
// API test:  curl -X POST http://localhost:3000/api/bookmarks \
//              -H 'Content-Type: application/json' \
//              -d '{"url":"https://rust-lang.org","title":"Rust","tag":"lang"}'

mod config;
mod db;
mod models;
mod repositories;
mod routes;
mod schema;
mod state;
mod tasks;

use autumn_cache_redis::RedisCachePlugin;
use autumn_web::migrate::{EmbeddedMigrations, embed_migrations};
use autumn_web::prelude::*;
use std::sync::Arc;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

fn build_distributed_state() -> Arc<state::DistributedState> {
    let config = config::DistributedConfig::load()
        .expect("distributed example config should load from autumn.toml");
    let pools =
        db::create_dual_pools(&config).expect("distributed example pools should build from config");

    Arc::new(state::DistributedState::new(config, pools))
        .install_global()
        .expect("distributed state should only be installed once")
}

#[autumn_web::main]
async fn main() {
    let distributed_state = build_distributed_state();
    tracing::info!(
        primary_url_configured = distributed_state.config.database.primary_url.is_some(),
        replica_url_configured = distributed_state.config.database.replica_url.is_some(),
        configured_primary_pool_size = distributed_state.config.database.primary_pool_size,
        configured_replica_pool_size = distributed_state.config.database.replica_pool_size,
        primary_pool_size = distributed_state.pools.primary_pool_size(),
        replica_pool_size = distributed_state.pools.replica_pool_size(),
        "installed distributed bookmarks state"
    );

    // -- v0.2: .tasks() registers scheduled background tasks -----
    autumn_web::app()
        .plugin(RedisCachePlugin::new())
        .migrations(MIGRATIONS)
        .routes(routes![
            routes::bookmarks::list,
            routes::bookmarks::by_tag,
            routes::bookmarks::new_form,
            routes::bookmarks::create,
            repositories::bookmark_api_count,
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    const MIGRATION_SQL: &str =
        include_str!("../migrations/00000000000000_create_bookmarks/up.sql");

    #[test]
    fn migration_uses_bigserial_ids() {
        assert!(
            MIGRATION_SQL.contains("id BIGSERIAL PRIMARY KEY"),
            "bookmark IDs must be 64-bit to match the Int8/i64 application schema",
        );
    }

    #[test]
    fn upgrade_migration_widens_existing_ids() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations/00000000000001_widen_bookmark_ids_to_bigint/up.sql");
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing upgrade migration at {}: {err}", path.display()));

        assert!(
            sql.contains("ALTER TABLE bookmarks ALTER COLUMN id TYPE BIGINT"),
            "bookmark upgrade migration must widen existing IDs to BIGINT",
        );
        assert!(
            sql.contains("ALTER SEQUENCE bookmarks_id_seq AS BIGINT"),
            "bookmark upgrade migration must widen the backing sequence to BIGINT",
        );
    }
}
