//! Feature Flag Plugin for Autumn
//!
//! Provides a real-time feature flag system.

use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

pub mod admin;
pub mod routes;

/// Trait for storing and retrieving feature flags.
pub trait FeatureFlagStore: Send + Sync + 'static {
    /// Retrieve all feature flags.
    fn get_all(&self) -> HashMap<String, bool>;
    /// Retrieve a specific feature flag.
    fn get(&self, name: &str) -> Option<bool>;
    /// Set a specific feature flag.
    fn set(&self, name: &str, value: bool);
}

/// In-memory implementation of [`FeatureFlagStore`].
#[derive(Default, Clone)]
pub struct InMemoryFeatureFlagStore {
    flags: Arc<tokio::sync::RwLock<HashMap<String, bool>>>,
}

impl InMemoryFeatureFlagStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl FeatureFlagStore for InMemoryFeatureFlagStore {
    fn get_all(&self) -> HashMap<String, bool> {
        self.flags.blocking_read().clone()
    }

    fn get(&self, name: &str) -> Option<bool> {
        self.flags.blocking_read().get(name).copied()
    }

    fn set(&self, name: &str, value: bool) {
        self.flags.blocking_write().insert(name.to_string(), value);
    }
}

/// The Feature Flag Plugin.
pub struct FeatureFlagPlugin {
    store: Arc<dyn FeatureFlagStore>,
}

impl FeatureFlagPlugin {
    pub fn new(store: Arc<dyn FeatureFlagStore>) -> Self {
        Self { store }
    }
}

impl Default for FeatureFlagPlugin {
    fn default() -> Self {
        Self::new(Arc::new(InMemoryFeatureFlagStore::new()))
    }
}

impl Plugin for FeatureFlagPlugin {
    fn name(&self) -> Cow<'static, str> {
        "autumn-feature-flag-plugin".into()
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let store = self.store.clone();

        app.with_extension(store)
            .declare_plugin_routes(routes::plugin_routes())
            .routes(routes::routes())
            .on_startup(|state| {
                let state = state;
                Box::pin(async move {
                    tracing::info!("Feature Flag plugin started");
                    // Ensure the channels topic is created early
                    let _ = state.channels().sender("feature-flags");
                    Ok(())
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_store() {
        let store = InMemoryFeatureFlagStore::new();

        assert_eq!(store.get("my_flag"), None);

        store.set("my_flag", true);
        assert_eq!(store.get("my_flag"), Some(true));

        store.set("my_flag", false);
        assert_eq!(store.get("my_flag"), Some(false));

        let all = store.get_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all.get("my_flag"), Some(&false));
    }
}
