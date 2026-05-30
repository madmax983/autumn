//! Feature-flag store for reddit-clone.
//!
//! Uses InMemoryFlagStore in dev/test and PgFlagStore in production (when a
//! primary database URL is present). Pre-configured flags:
//!
//! | Flag              | Default          | Purpose                                   |
//! |-------------------|------------------|-------------------------------------------|
//! | `new_ui_preview`  | 25% rollout      | Shows the "New UI" banner to early testers|
//! | `post_awards`     | off              | Enables the Awards widget on post pages   |

use autumn_web::config::AutumnConfig;
use autumn_web::feature_flags::{FlagStore, InMemoryFlagStore, pg::PgFlagStore};

/// Build the flag store appropriate for the current environment.
pub fn build_store(config: &AutumnConfig) -> Box<dyn FlagStore> {
    if let Some(url) = config.database.effective_primary_url() {
        let store = PgFlagStore::new(url);
        configure(&store);
        return Box::new(store);
    }

    let store = InMemoryFlagStore::new();
    configure(&store);
    Box::new(store)
}

fn configure(store: &dyn FlagStore) {
    // Seed defaults only when the flag is absent so that runtime changes
    // (e.g. `autumn flags enable post_awards`) survive restarts/redeploys.
    if store.get("new_ui_preview").ok().flatten().is_none() {
        // 25 % rollout — stable per (flag_name, actor_id).
        store.set_rollout("new_ui_preview", 25, Some("init")).ok();
    }
    if store.get("post_awards").ok().flatten().is_none() {
        // Off by default; enable with: autumn flags enable post_awards
        store.disable("post_awards", Some("init")).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_store_without_database_returns_in_memory_store() {
        let config = AutumnConfig::default();
        let store = build_store(&config);
        // Should work without panicking.
        let _ = store.list().unwrap();
    }

    #[test]
    fn new_ui_preview_defaults_to_25_pct_rollout() {
        let config = AutumnConfig::default();
        let store = build_store(&config);
        let flag = store.get("new_ui_preview").unwrap().unwrap();
        assert_eq!(flag.rollout_pct, 25);
    }

    #[test]
    fn post_awards_defaults_to_disabled() {
        let config = AutumnConfig::default();
        let store = build_store(&config);
        let flag = store.get("post_awards").unwrap().unwrap();
        assert!(!flag.enabled);
    }
}
