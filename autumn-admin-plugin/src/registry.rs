//! Runtime registry that collects admin-enabled models.
//!
//! The [`AdminRegistry`] is built during plugin construction and stored
//! in [`AppState`](autumn_web::AppState) via `with_extension`. Route handlers retrieve it to
//! discover which models are available and how to render them.

use std::collections::btree_map::{BTreeMap, Entry};

use crate::traits::AdminModel;

/// Holds all registered admin models, keyed by their URL slug.
///
/// Stored as an `Arc<AdminRegistry>` in `AppState` extensions so route
/// handlers can access it cheaply.
pub struct AdminRegistry {
    /// Models keyed by slug, ordered alphabetically for consistent nav.
    models: BTreeMap<&'static str, Box<dyn AdminModel>>,
}

impl AdminRegistry {
    /// Create an empty registry.
    pub(crate) fn new() -> Self {
        Self {
            models: BTreeMap::new(),
        }
    }

    /// Register a model. Panics on duplicate slugs (catches config bugs
    /// at startup rather than silently shadowing).
    pub(crate) fn register<M: AdminModel>(&mut self, model: M) {
        let slug = model.slug();
        let name = model.display_name();
        match self.models.entry(slug) {
            Entry::Occupied(_) => panic!(
                "autumn-admin: duplicate model slug '{slug}' — each model must have a unique slug",
            ),
            Entry::Vacant(e) => {
                tracing::debug!(slug, name, "Registered admin model");
                e.insert(Box::new(model));
            }
        }
    }

    /// Number of registered models.
    #[must_use]
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Get a model by its URL slug.
    #[must_use]
    pub fn get(&self, slug: &str) -> Option<&dyn AdminModel> {
        self.models.get(slug).map(Box::as_ref)
    }

    /// Iterate over all registered models in alphabetical order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &dyn AdminModel)> {
        self.models
            .iter()
            .map(|(&slug, model)| (slug, model.as_ref()))
    }

    /// Get all slugs (for nav rendering).
    #[must_use]
    pub fn slugs(&self) -> Vec<&'static str> {
        self.models.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::*;
    use serde_json::Value;

    struct DummyModel {
        slug: &'static str,
        name: &'static str,
    }

    impl AdminModel for DummyModel {
        fn slug(&self) -> &'static str {
            self.slug
        }
        fn display_name(&self) -> &'static str {
            self.name
        }
        fn display_name_plural(&self) -> &'static str {
            self.name
        }
        fn fields(&self) -> Vec<AdminField> {
            vec![]
        }
        fn list(
            &self,
            _pool: &diesel_async::pooled_connection::deadpool::Pool<
                diesel_async::AsyncPgConnection,
            >,
            _params: ListParams,
        ) -> AdminFuture<'_, ListResult> {
            Box::pin(async {
                Ok(ListResult {
                    records: vec![],
                    total: 0,
                    page: 1,
                    per_page: 25,
                })
            })
        }
        fn get(
            &self,
            _pool: &diesel_async::pooled_connection::deadpool::Pool<
                diesel_async::AsyncPgConnection,
            >,
            _id: i64,
        ) -> AdminFuture<'_, Option<Value>> {
            Box::pin(async { Ok(None) })
        }
        fn create(
            &self,
            _pool: &diesel_async::pooled_connection::deadpool::Pool<
                diesel_async::AsyncPgConnection,
            >,
            data: Value,
        ) -> AdminFuture<'_, Value> {
            Box::pin(async move { Ok(data) })
        }
        fn update(
            &self,
            _pool: &diesel_async::pooled_connection::deadpool::Pool<
                diesel_async::AsyncPgConnection,
            >,
            _id: i64,
            data: Value,
        ) -> AdminFuture<'_, Value> {
            Box::pin(async move { Ok(data) })
        }
        fn delete(
            &self,
            _pool: &diesel_async::pooled_connection::deadpool::Pool<
                diesel_async::AsyncPgConnection,
            >,
            _id: i64,
        ) -> AdminFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn register_and_retrieve() {
        let mut registry = AdminRegistry::new();
        registry.register(DummyModel {
            slug: "projects",
            name: "Project",
        });
        assert_eq!(registry.model_count(), 1);
        assert!(registry.get("projects").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn iter_is_sorted() {
        let mut registry = AdminRegistry::new();
        registry.register(DummyModel {
            slug: "tickets",
            name: "Ticket",
        });
        registry.register(DummyModel {
            slug: "projects",
            name: "Project",
        });
        let slugs: Vec<_> = registry.iter().map(|(s, _)| s).collect();
        assert_eq!(slugs, vec!["projects", "tickets"]);
    }

    #[test]
    #[should_panic(expected = "duplicate model slug")]
    fn duplicate_slug_panics() {
        let mut registry = AdminRegistry::new();
        registry.register(DummyModel {
            slug: "projects",
            name: "Project",
        });
        registry.register(DummyModel {
            slug: "projects",
            name: "Project 2",
        });
    }
}
